#!/usr/bin/env python3
"""
LoRA fine-tune of a small instruct model into OSAIMA's intent parser.

Runs on CUDA when available (bf16), otherwise on CPU (fp32). Only the answer
tokens are trained on; the short system prompt and the request are masked out.
At the end the adapter is merged into the base weights and saved as a plain
Hugging Face model, ready for export.sh (GGUF Q4_K_M).

    python train.py                                   # Qwen2.5-0.5B-Instruct, 3 epochs
    python train.py --model Qwen/Qwen2.5-1.5B-Instruct --out out/qwen1.5b
    python train.py --max-steps 200                   # quick CPU sanity run
"""
import argparse
import json
import math
import os
import random
import time

import torch

import fmt

HERE = os.path.dirname(os.path.abspath(__file__))


def load(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def encode(tok, row, max_len):
    prompt = tok.apply_chat_template(fmt.messages(row["request"], row["now"]), tokenize=False, add_generation_prompt=True)
    answer = fmt.target_text(row["target"]) + "<|im_end|>\n"
    p = tok(prompt, add_special_tokens=False)["input_ids"]
    a = tok(answer, add_special_tokens=False)["input_ids"]
    ids = (p + a)[:max_len]
    labels = ([-100] * len(p) + a)[:max_len]
    return ids, labels


def batches(examples, size, pad_id, shuffle, rng):
    order = list(range(len(examples)))
    if shuffle:
        rng.shuffle(order)
    for i in range(0, len(order), size):
        chunk = [examples[j] for j in order[i:i + size]]
        n = max(len(ids) for ids, _ in chunk)
        ids = torch.full((len(chunk), n), pad_id)
        labels = torch.full((len(chunk), n), -100)
        mask = torch.zeros((len(chunk), n), dtype=torch.long)
        for k, (x, y) in enumerate(chunk):
            ids[k, :len(x)] = torch.tensor(x)
            labels[k, :len(y)] = torch.tensor(y)
            mask[k, :len(x)] = 1
        yield ids, labels, mask


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="Qwen/Qwen2.5-0.5B-Instruct")
    ap.add_argument("--data", default=os.path.join(HERE, "data", "train.jsonl"))
    ap.add_argument("--out", default=os.path.join(HERE, "out", "qwen0.5b"))
    ap.add_argument("--epochs", type=float, default=3)
    ap.add_argument("--max-steps", type=int, default=0, help="stop after this many optimizer steps (0 = all epochs)")
    ap.add_argument("--batch", type=int, default=0, help="examples per step (default 16 on GPU, 8 on CPU)")
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--rank", type=int, default=16)
    ap.add_argument("--max-len", type=int, default=640)
    ap.add_argument("--threads", type=int, default=0)
    ap.add_argument("--no-merge", action="store_true")
    ap.add_argument("--unsloth", action="store_true", help="load and patch the model with Unsloth (CUDA only)")
    args = ap.parse_args()

    from peft import LoraConfig, get_peft_model
    from transformers import AutoModelForCausalLM, AutoTokenizer

    cuda = torch.cuda.is_available()
    device = "cuda" if cuda else "cpu"
    dtype = torch.bfloat16 if cuda and torch.cuda.is_bf16_supported() else (torch.float16 if cuda else torch.float32)
    if not cuda:
        # a shared desktop: leave most cores to everything else unless asked
        torch.set_num_threads(args.threads or 2)
    batch = args.batch or (16 if cuda else 8)
    rng = random.Random(0)
    torch.manual_seed(0)

    targets = ["q_proj", "k_proj", "v_proj", "o_proj", "gate_proj", "up_proj", "down_proj"]
    if args.unsloth:
        # Unsloth's fused kernels: ~2x faster on a T4/RTX; same LoRA, same loop below
        from unsloth import FastLanguageModel
        model, tok = FastLanguageModel.from_pretrained(args.model, max_seq_length=args.max_len, dtype=None, load_in_4bit=False)
        model = FastLanguageModel.get_peft_model(model, r=args.rank, lora_alpha=2 * args.rank, lora_dropout=0,
                                                 target_modules=targets, use_gradient_checkpointing="unsloth")
        dtype = next(p.dtype for p in model.parameters() if p.is_floating_point())
        if dtype == torch.float32:
            dtype = torch.float16
    else:
        tok = AutoTokenizer.from_pretrained(args.model)
        model = AutoModelForCausalLM.from_pretrained(args.model, dtype=torch.float32 if dtype == torch.float16 else dtype)
        model.to(device)
        model.gradient_checkpointing_enable()
        model.enable_input_require_grads()
        model.config.use_cache = False
        model = get_peft_model(model, LoraConfig(
            r=args.rank, lora_alpha=2 * args.rank, lora_dropout=0.05, task_type="CAUSAL_LM", target_modules=targets))
    model.print_trainable_parameters()

    rows = load(args.data)
    examples = [encode(tok, r, args.max_len) for r in rows]
    # similar lengths per batch waste less padding: sort into buckets, shuffle the buckets
    lens = [len(x) for x, _ in examples]
    print(f"{len(examples)} examples, tokens/example mean {sum(lens) / len(lens):.0f} max {max(lens)}, "
          f"answer tokens mean {sum(sum(1 for t in y if t != -100) for _, y in examples) / len(examples):.0f}")

    steps_per_epoch = math.ceil(len(examples) / batch)
    total = args.max_steps or int(steps_per_epoch * args.epochs)
    params = [p for p in model.parameters() if p.requires_grad]
    for p in params:  # the adapter trains in fp32 (mixed precision), whatever the base weights are
        p.data = p.data.float()
    opt = torch.optim.AdamW(params, lr=args.lr, weight_decay=0.0)
    warm = max(1, total // 20)
    sched = torch.optim.lr_scheduler.LambdaLR(
        opt, lambda s: min(1.0, (s + 1) / warm) * 0.5 * (1 + math.cos(math.pi * min(1.0, s / max(1, total)))))
    scaler = torch.amp.GradScaler("cuda") if dtype == torch.float16 else None

    model.train()
    step, start, run_loss = 0, time.time(), 0.0
    while step < total:
        for ids, labels, mask in batches(examples, batch, tok.pad_token_id or 0, True, rng):
            ids, labels, mask = ids.to(device), labels.to(device), mask.to(device)
            with torch.autocast(device_type=device, dtype=dtype, enabled=cuda):
                loss = model(input_ids=ids, attention_mask=mask, labels=labels).loss
            if scaler:
                scaler.scale(loss).backward()
                scaler.unscale_(opt)
                torch.nn.utils.clip_grad_norm_(params, 1.0)
                scaler.step(opt)
                scaler.update()
            else:
                loss.backward()
                torch.nn.utils.clip_grad_norm_(params, 1.0)
                opt.step()
            opt.zero_grad(set_to_none=True)
            sched.step()
            step += 1
            run_loss = loss.item() if step == 1 else 0.9 * run_loss + 0.1 * loss.item()
            if step % 10 == 0 or step == total:
                el = time.time() - start
                print(f"step {step}/{total} epoch {step / steps_per_epoch:.2f} loss {run_loss:.4f} "
                      f"{el / step:.2f}s/step eta {el / step * (total - step) / 60:.1f} min", flush=True)
            if step >= total:
                break

    os.makedirs(args.out, exist_ok=True)
    model.save_pretrained(os.path.join(args.out, "adapter"))
    info = {"base": args.model, "examples": len(examples), "steps": step, "batch": batch, "lr": args.lr,
            "rank": args.rank, "device": device, "minutes": round((time.time() - start) / 60, 1), "final_loss": run_loss}
    with open(os.path.join(args.out, "train_info.json"), "w") as f:
        json.dump(info, f, indent=2)
    print(info)
    if not args.no_merge:
        merged = model.merge_and_unload()
        merged = merged.to(torch.float16 if cuda else torch.float32)
        merged.save_pretrained(os.path.join(args.out, "merged"), safe_serialization=True)
        tok.save_pretrained(os.path.join(args.out, "merged"))
        print("merged model saved to", os.path.join(args.out, "merged"))


if __name__ == "__main__":
    main()
