#!/usr/bin/env python3
"""
Export the intent router for deployment without PyTorch: the MiniLM encoder as
ONNX (fp32, plus an int8 variant), its tokenizer, and the logistic-regression
head as plain arrays. The router uses fp32 by default: on CPUs without VNNI
(e.g. Ryzen 3550H) int8 was no faster and lost ~1.7 points of fast-path accuracy.
router_service.py then needs only onnxruntime, tokenizers and numpy (~100 MB
installed instead of ~1 GB for torch).

Usage:  python export_onnx.py        (after train_embed_classifier.py)
Output: intent_embed/encoder.onnx, encoder.int8.onnx, tokenizer.json, head.npz
"""
import os

import joblib
import numpy as np
import torch
from transformers import AutoModel, AutoTokenizer

OUT = "intent_embed"


class MeanPooled(torch.nn.Module):
    """all-MiniLM-L6-v2's sentence embedding: mean pooling, then L2 normalisation."""

    def __init__(self, name):
        super().__init__()
        self.bert = AutoModel.from_pretrained(name)

    def forward(self, input_ids, attention_mask, token_type_ids):
        hidden = self.bert(input_ids=input_ids, attention_mask=attention_mask,
                           token_type_ids=token_type_ids).last_hidden_state
        mask = attention_mask.unsqueeze(-1).to(hidden.dtype)
        pooled = (hidden * mask).sum(1) / mask.sum(1).clamp(min=1e-9)
        return torch.nn.functional.normalize(pooled, p=2, dim=1)


def main():
    bundle = joblib.load(os.path.join(OUT, "head.joblib"))
    name, head = bundle["model"], bundle["head"]

    tok = AutoTokenizer.from_pretrained(name)
    tok.backend_tokenizer.save(os.path.join(OUT, "tokenizer.json"))
    np.savez(os.path.join(OUT, "head.npz"), coef=head.coef_.astype(np.float32),
             intercept=head.intercept_.astype(np.float32), classes=np.array(head.classes_))

    model = MeanPooled(name).eval()
    sample = tok(["open the terminal"], return_tensors="pt")
    fp32 = os.path.join(OUT, "encoder.onnx")
    torch.onnx.export(
        model, (sample["input_ids"], sample["attention_mask"], sample["token_type_ids"]), fp32,
        input_names=["input_ids", "attention_mask", "token_type_ids"], output_names=["embedding"],
        dynamic_axes={k: {0: "batch", 1: "seq"} for k in ("input_ids", "attention_mask", "token_type_ids")}
        | {"embedding": {0: "batch"}},
        opset_version=17, dynamo=False)

    from onnxruntime.quantization import QuantType, quantize_dynamic
    int8 = os.path.join(OUT, "encoder.int8.onnx")
    quantize_dynamic(fp32, int8, weight_type=QuantType.QInt8)
    for p in (fp32, int8):
        print(f"{p}: {os.path.getsize(p) / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
