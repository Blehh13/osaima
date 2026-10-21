#!/usr/bin/env python3
"""
Fine-tune BERT-tiny for OSAIMA intent classification (PDR §3.3, §5.4 steps 15-16).

Classifies an utterance into one of 40 intents (the 3-track label is derived from
the intent via labels.json, so one head covers both). Target: >92% accuracy on
the held-out test set; model ~15 MB; ~30 ms CPU inference; exportable to ONNX.

Prereqs:
    pip install transformers torch scikit-learn datasets
    python split_dataset.py          # produces train/val/test.jsonl + labels.json

Usage:
    python train_bert_tiny.py                    # train + evaluate
    python train_bert_tiny.py --export-onnx      # also write model.onnx
    python train_bert_tiny.py --predict "open firefox"   # load + classify one utterance

Base models (--base):
  bert-tiny  prajjwal1/bert-tiny, 2 layers, ~4.4M params, ~17 MB (the PDR's choice)
  minilm     sentence-transformers/all-MiniLM-L6-v2, 6 layers, ~22M params, ~90 MB;
             pretrained on paraphrase pairs, and the same model the RAG engine loads

The reported test score is on testset.jsonl (held-out phrasings, see
generate_testset.py), not on the random split of the training templates.
"""
import argparse, json, os, sys

MODEL_DIR = "intent_bert_tiny"
BASE = "prajjwal1/bert-tiny"
# bert-tiny ships no tokenizer.json (transformers 5.x can't build its fast tokenizer);
# use bert-base-uncased's tokenizer — identical 30522 WordPiece vocab, fully compatible.
TOKENIZER = "bert-base-uncased"
MINILM = "sentence-transformers/all-MiniLM-L6-v2"
HELDOUT = "testset.jsonl"

def load_jsonl(p):
    with open(p, encoding="utf-8") as f:
        return [json.loads(l) for l in f if l.strip()]

def load_labels():
    with open("labels.json", encoding="utf-8") as f:
        return json.load(f)

def train(args):
    import torch
    from torch.utils.data import Dataset
    from transformers import (AutoTokenizer, AutoModelForSequenceClassification,
                              TrainingArguments, Trainer)
    from sklearn.metrics import accuracy_score, f1_score

    labels = load_labels()
    intent2id = labels["intent2id"]
    id2intent = {int(k): v for k, v in labels["id2intent"].items()}
    n_labels = len(intent2id)

    tok = AutoTokenizer.from_pretrained(MINILM if args.base == "minilm" else TOKENIZER)

    class DS(Dataset):
        def __init__(self, rows):
            self.rows = rows
        def __len__(self): return len(self.rows)
        def __getitem__(self, i):
            r = self.rows[i]
            enc = tok(r["utterance"], truncation=True, padding="max_length", max_length=32)
            enc["labels"] = intent2id[r["intent"]]
            return {k: torch.tensor(v) for k, v in enc.items()}

    train_ds = DS(load_jsonl("train.jsonl"))
    val_ds   = DS(load_jsonl("val.jsonl"))
    test_ds  = DS(load_jsonl("test.jsonl"))
    heldout = [r for r in load_jsonl(HELDOUT) if r["intent"] in intent2id] if os.path.exists(HELDOUT) else []

    # bert-tiny's config.json lacks a `model_type` key, so AutoModel rejects it under
    # transformers 5.x. Build the tiny BERT config explicitly and load the pretrained
    # weights with the Bert* class (bypasses AutoConfig). Falls back to from-scratch
    # init if the pretrained weights can't be fetched.
    from transformers import BertConfig, BertForSequenceClassification
    if args.base == "minilm":
        model = BertForSequenceClassification.from_pretrained(
            MINILM, num_labels=n_labels, id2label=id2intent, label2id=intent2id)
    cfg = BertConfig(
        vocab_size=30522, hidden_size=128, num_hidden_layers=2,
        num_attention_heads=2, intermediate_size=512, max_position_embeddings=512,
        num_labels=n_labels, id2label=id2intent, label2id=intent2id,
    )
    if args.base == "bert-tiny":
        try:
            model = BertForSequenceClassification.from_pretrained(BASE, config=cfg)
            print("loaded pretrained bert-tiny weights")
        except Exception as e:
            print(f"pretrained load failed ({e}); training from scratch (still fine at this scale)")
            model = BertForSequenceClassification(cfg)

    def metrics(p):
        preds = p.predictions.argmax(-1)
        return {"accuracy": accuracy_score(p.label_ids, preds),
                "f1_macro": f1_score(p.label_ids, preds, average="macro")}

    targs = TrainingArguments(
        output_dir=args.out, num_train_epochs=args.epochs,
        per_device_train_batch_size=64, per_device_eval_batch_size=128,
        learning_rate=5e-4 if args.base == "bert-tiny" else 5e-5,
        eval_strategy="epoch", save_strategy="epoch", save_total_limit=1,
        load_best_model_at_end=True, metric_for_best_model="accuracy",
        logging_steps=50, report_to=[])

    trainer = Trainer(model=model, args=targs, train_dataset=train_ds,
                      eval_dataset=val_ds, compute_metrics=metrics)
    trainer.train()

    print("\n== IN-DISTRIBUTION SPLIT (same templates as training) ==")
    print(trainer.evaluate(test_ds))
    if heldout:
        print(f"\n== HELD-OUT {HELDOUT} ({len(heldout)} rows, unseen phrasings; PDR target > 0.92) ==")
        print(trainer.evaluate(DS(heldout)))
    trainer.save_model(args.out); tok.save_pretrained(args.out)
    import glob, shutil
    for ckpt in glob.glob(os.path.join(args.out, "checkpoint-*")):
        shutil.rmtree(ckpt)  # the final model is saved above; checkpoints only waste space
    print(f"Saved -> {args.out}/")

    if args.export_onnx:
        import subprocess  # list-form args (no shell) — MODEL_DIR is a fixed constant
        subprocess.run(["optimum-cli", "export", "onnx", "--model", args.out,
                        f"{args.out}_onnx"], check=True)
        print(f"ONNX -> {args.out}_onnx/  (faster CPU inference)")

def predict(text):
    import torch
    from transformers import AutoTokenizer, AutoModelForSequenceClassification
    if not os.path.isdir(MODEL_DIR):
        sys.exit(f"train first — {MODEL_DIR}/ not found")
    labels = load_labels()
    id2track = {v: k for k, v in {i: t for i, t in
                enumerate([labels['id2track'][str(i)] for i in range(len(labels['track2id']))])}.items()}
    tok = AutoTokenizer.from_pretrained(MODEL_DIR, use_fast=False)
    model = AutoModelForSequenceClassification.from_pretrained(MODEL_DIR)
    enc = tok(text, return_tensors="pt", truncation=True, max_length=32)
    with torch.no_grad():
        logits = model(**enc).logits
    intent = model.config.id2label[int(logits.argmax(-1))]
    # map intent -> track via the taxonomy in generate_dataset_llm if present
    print(json.dumps({"utterance": text, "intent": intent}, ensure_ascii=False))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", choices=["bert-tiny", "minilm"], default="bert-tiny")
    ap.add_argument("--out", help="output dir (default: intent_bert_tiny or intent_minilm)")
    ap.add_argument("--epochs", type=int, default=5)
    ap.add_argument("--export-onnx", action="store_true")
    ap.add_argument("--predict", metavar="TEXT")
    args = ap.parse_args()
    args.out = args.out or ("intent_minilm" if args.base == "minilm" else MODEL_DIR)
    if args.predict: predict(args.predict)
    else: train(args)

if __name__ == "__main__":
    main()
