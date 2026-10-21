# Process classifier

Trained on 1440 one-window measurements of synthetic programs (background: 360, batch: 360, interactive: 360, media: 360).

| | Accuracy (5-fold cross-validation) |
|---|---|
| Trained model | 91.4% |
| Built-in rules | 84.2% |

## Per class (trained model)

```
              precision    recall  f1-score   support

  background      1.000     1.000     1.000       360
       batch      0.782     0.908     0.841       360
 interactive      1.000     1.000     1.000       360
       media      0.891     0.747     0.813       360

    accuracy                          0.914      1440
   macro avg      0.918     0.914     0.913      1440
weighted avg      0.918     0.914     0.913      1440

```

## Confusion matrix (trained model)

| actual \ predicted | background | batch | interactive | media |
|---|---|---|---|---|
| background | 360 | 0 | 0 | 0 |
| batch | 0 | 327 | 0 | 33 |
| interactive | 0 | 0 | 360 | 0 |
| media | 0 | 91 | 0 | 269 |


## Confusion matrix (built-in rules)

| actual \ predicted | background | batch | interactive | media |
|---|---|---|---|---|
| background | 360 | 0 | 0 | 0 |
| batch | 0 | 360 | 0 | 0 |
| interactive | 0 | 0 | 360 | 0 |
| media | 0 | 216 | 11 | 133 |


## Weights (standardised inputs; positive pushes towards the class)

| class | cpu_util | vol_rate | invol_rate | slice_ms | wait_ratio | bias |
|---|---|---|---|---|---|---|
| background | -0.95 | -2.77 | -5.33 | -2.93 | +0.10 | -1.32 |
| batch | +6.65 | -1.19 | +0.97 | +0.82 | -0.26 | +1.57 |
| interactive | -8.67 | +1.56 | +0.03 | -0.37 | +0.52 | -1.54 |
| media | +2.97 | +2.39 | +4.33 | +2.48 | -0.36 | +1.29 |
