#!/usr/bin/env python3
"""Train the process classifier from the data `collect.py` wrote.

A multinomial logistic regression on the five features, standardised. It is evaluated
with 5-fold cross-validation and compared against the built-in rules, then saved as
JSON for the daemon (`model.json`) together with a few worked examples (`golden.json`)
that a Rust test checks against, so the daemon is known to compute the same answers.

  train.py --data dataset.csv --out model/
"""

import argparse
import csv
import json
import os
import sys

import numpy as np
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, classification_report, confusion_matrix
from sklearn.model_selection import StratifiedKFold, cross_val_predict
from sklearn.pipeline import make_pipeline
from sklearn.preprocessing import StandardScaler

FEATURES = ["cpu_util", "vol_rate", "invol_rate", "slice_ms", "wait_ratio"]


def model_inputs(raw: np.ndarray) -> np.ndarray:
    """What the daemon feeds its model: cpu_util as is, the rest squeezed with ln(1 + x)."""
    x = raw.astype(float).copy()
    x[:, 1:] = np.log1p(np.maximum(raw[:, 1:], 0.0))
    return x


def rules(raw: np.ndarray) -> list[str]:
    """The daemon's built-in rules (src/model.rs), for comparison."""
    out = []
    for cpu, vol, _invol, slice_ms, _wait in raw:
        if cpu < 0.02:
            out.append("background")
        elif cpu >= 0.6 and vol < 30:
            out.append("batch")
        elif cpu < 0.25 and slice_ms < 4:
            out.append("interactive")
        else:
            out.append("media")
    return out


def load(path: str) -> tuple[np.ndarray, np.ndarray]:
    with open(path, newline="") as handle:
        rows = list(csv.DictReader(handle))
    labels = np.array([r["label"] for r in rows])
    raw = np.array([[float(r[f]) for f in FEATURES] for r in rows])
    return raw, labels


def table(matrix: np.ndarray, classes: list[str]) -> str:
    head = "| actual \\ predicted | " + " | ".join(classes) + " |\n|---|" + "---|" * len(classes) + "\n"
    return head + "".join(f"| {c} | " + " | ".join(str(int(v)) for v in row) + " |\n" for c, row in zip(classes, matrix))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True)
    parser.add_argument("--out", required=True, help="folder for model.json, golden.json and report.md")
    args = parser.parse_args()

    raw, labels = load(args.data)
    classes = sorted(set(labels))
    counts = {c: int((labels == c).sum()) for c in classes}
    print("rows per class:", counts)
    if len(classes) < 2 or min(counts.values()) < 10:
        print("not enough data to train", file=sys.stderr)
        return 1

    x = model_inputs(raw)
    pipeline = make_pipeline(StandardScaler(), LogisticRegression(C=10.0, max_iter=5000))
    folds = StratifiedKFold(n_splits=5, shuffle=True, random_state=1)
    predicted = cross_val_predict(pipeline, x, labels, cv=folds)
    accuracy = accuracy_score(labels, predicted)
    baseline = accuracy_score(labels, rules(raw))
    print(f"cross-validated accuracy: {accuracy:.3f}   built-in rules: {baseline:.3f}")

    pipeline.fit(x, labels)
    scaler, clf = pipeline.named_steps["standardscaler"], pipeline.named_steps["logisticregression"]
    model = {
        "version": 1,
        "classes": list(clf.classes_),
        "mean": [round(float(v), 10) for v in scaler.mean_],
        "scale": [round(float(v), 10) for v in scaler.scale_],
        "weights": [[round(float(v), 10) for v in row] for row in clf.coef_],
        "bias": [round(float(v), 10) for v in clf.intercept_],
    }

    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "model.json"), "w") as handle:
        json.dump(model, handle, indent=2)
        handle.write("\n")

    # worked examples for the Rust test: raw features, and the answer scikit-learn gives
    picks = np.linspace(0, len(raw) - 1, num=min(24, len(raw)), dtype=int)
    probabilities = pipeline.predict_proba(x[picks])
    golden = [
        {
            "features": dict(zip(FEATURES, (float(v) for v in raw[i]))),
            "class": str(clf.classes_[int(np.argmax(p))]),
            "probability": float(np.max(p)),
            "probabilities": {str(c): float(v) for c, v in zip(clf.classes_, p)},
        }
        for i, p in zip(picks, probabilities)
    ]
    with open(os.path.join(args.out, "golden.json"), "w") as handle:
        json.dump(golden, handle, indent=2)
        handle.write("\n")

    report = [
        "# Process classifier",
        "",
        f"Trained on {len(raw)} one-window measurements of synthetic programs "
        f"({', '.join(f'{c}: {n}' for c, n in counts.items())}).",
        "",
        "| | Accuracy (5-fold cross-validation) |",
        "|---|---|",
        f"| Trained model | {accuracy:.1%} |",
        f"| Built-in rules | {baseline:.1%} |",
        "",
        "## Per class (trained model)",
        "",
        "```",
        classification_report(labels, predicted, digits=3),
        "```",
        "",
        "## Confusion matrix (trained model)",
        "",
        table(confusion_matrix(labels, predicted, labels=classes), classes),
        "",
        "## Confusion matrix (built-in rules)",
        "",
        table(confusion_matrix(labels, rules(raw), labels=classes), classes),
        "",
        "## Weights (standardised inputs; positive pushes towards the class)",
        "",
        "| class | " + " | ".join(FEATURES) + " | bias |",
        "|---|" + "---|" * (len(FEATURES) + 1),
        *[f"| {c} | " + " | ".join(f"{w:+.2f}" for w in row) + f" | {b:+.2f} |" for c, row, b in zip(clf.classes_, clf.coef_, clf.intercept_)],
        "",
    ]
    with open(os.path.join(args.out, "report.md"), "w") as handle:
        handle.write("\n".join(report))
    print("\n".join(report))
    return 0


if __name__ == "__main__":
    sys.exit(main())
