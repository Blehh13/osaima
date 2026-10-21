# Process classifier

Trained on 1200 one-window measurements of synthetic programs (background: 300, batch: 300, interactive: 300, media: 300).

| | Accuracy (5-fold cross-validation) |
|---|---|
| Trained model | 98.6% |
| Built-in rules | 72.1% |

## Per class (trained model)

```
              precision    recall  f1-score   support

  background      1.000     1.000     1.000       300
       batch      0.961     0.983     0.972       300
 interactive      1.000     1.000     1.000       300
       media      0.983     0.960     0.971       300

    accuracy                          0.986      1200
   macro avg      0.986     0.986     0.986      1200
weighted avg      0.986     0.986     0.986      1200

```

## Confusion matrix (trained model)

| actual \ predicted | background | batch | interactive | media |
|---|---|---|---|---|
| background | 300 | 0 | 0 | 0 |
| batch | 0 | 295 | 0 | 5 |
| interactive | 0 | 0 | 300 | 0 |
| media | 0 | 12 | 0 | 288 |


## Confusion matrix (built-in rules)

| actual \ predicted | background | batch | interactive | media |
|---|---|---|---|---|
| background | 300 | 0 | 0 | 0 |
| batch | 0 | 25 | 0 | 275 |
| interactive | 0 | 0 | 300 | 0 |
| media | 0 | 0 | 60 | 240 |


## Weights (standardised inputs; positive pushes towards the class)

| class | cpu_util | vol_rate | invol_rate | slice_ms | wait_ratio | bias |
|---|---|---|---|---|---|---|
| background | -1.36 | -2.74 | -3.92 | -3.05 | +1.04 | +0.24 |
| batch | +4.48 | -2.38 | +1.59 | -0.11 | -5.50 | -1.27 |
| interactive | -6.23 | +1.09 | +0.58 | +0.81 | +2.02 | +0.74 |
| media | +3.11 | +4.03 | +1.75 | +2.35 | +2.44 | +0.30 |
