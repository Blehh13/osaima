//! Deciding what kind of process this is.
//!
//! The classifier is a multinomial logistic regression: a small table of numbers
//! trained offline (`train/train.py`) on real workloads measured by this very
//! program, saved as JSON, and evaluated here with a few multiplications. When no
//! trained model is available, hand-written rules give a sensible answer instead.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::features::Features;

/// The kinds of process the scheduler tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    /// Waits for the user and answers quickly: editors, shells, the desktop itself.
    Interactive,
    /// Does regular work against a deadline: video and audio playback, games, animation.
    Media,
    /// Wants every CPU cycle it can get and does not mind waiting: compilers, encoders.
    Batch,
    /// Almost never runs.
    Background,
}

impl Class {
    pub const ALL: [Class; 4] = [
        Class::Interactive,
        Class::Media,
        Class::Batch,
        Class::Background,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Class::Interactive => "interactive",
            Class::Media => "media",
            Class::Batch => "batch",
            Class::Background => "background",
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Hand-written rules, used when there is no trained model. Thresholds are in cores of
/// CPU, switches per second and milliseconds (see [`Features`]).
///
/// They judge by demand, the CPU a process would use if it never had to wait, not by what
/// it got: on a crowded machine a compiler gets a third of a core and would otherwise look
/// like light work.
pub fn rules(f: &Features) -> Class {
    let demand = f.cpu_util * (1.0 + f.wait_ratio);
    if demand < 0.02 {
        Class::Background
    } else if demand >= 0.6 && f.vol_rate < 30.0 {
        Class::Batch
    } else if demand < 0.25 && f.slice_ms < 4.0 {
        Class::Interactive
    } else {
        Class::Media
    }
}

/// A trained logistic regression.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Model {
    pub version: u32,
    /// The kind of process each row of `weights` scores.
    pub classes: Vec<Class>,
    /// Mean and spread of each input in the training data (inputs are standardised).
    pub mean: Vec<f64>,
    pub scale: Vec<f64>,
    /// One row of weights per class, one weight per input.
    pub weights: Vec<Vec<f64>>,
    pub bias: Vec<f64>,
}

#[derive(Debug, PartialEq)]
pub enum ModelError {
    Parse(String),
    Shape(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Parse(m) => write!(f, "the model file is not valid: {m}"),
            ModelError::Shape(m) => write!(f, "the model does not fit: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}

const INPUTS: usize = 5;

impl Model {
    pub fn from_json(text: &str) -> Result<Model, ModelError> {
        let model: Model =
            serde_json::from_str(text).map_err(|e| ModelError::Parse(e.to_string()))?;
        model.check()?;
        Ok(model)
    }

    fn check(&self) -> Result<(), ModelError> {
        let k = self.classes.len();
        if k < 2 {
            return Err(ModelError::Shape("it needs at least two classes".into()));
        }
        if self.weights.len() != k || self.bias.len() != k {
            return Err(ModelError::Shape(format!(
                "{k} classes but {} weight rows and {} biases",
                self.weights.len(),
                self.bias.len()
            )));
        }
        if self.mean.len() != INPUTS
            || self.scale.len() != INPUTS
            || self.weights.iter().any(|w| w.len() != INPUTS)
        {
            return Err(ModelError::Shape(format!(
                "every row must have {INPUTS} values"
            )));
        }
        if self.scale.iter().any(|s| !s.is_finite() || *s <= 0.0) {
            return Err(ModelError::Shape(
                "every scale must be a positive number".into(),
            ));
        }
        let mut all = self
            .mean
            .iter()
            .chain(self.bias.iter())
            .chain(self.weights.iter().flatten());
        if all.any(|v| !v.is_finite()) {
            return Err(ModelError::Shape(
                "a number in the model is not finite".into(),
            ));
        }
        Ok(())
    }

    /// The most likely class and how sure the model is (0.5 to 1.0 for two classes, lower
    /// with more).
    pub fn predict(&self, f: &Features) -> (Class, f64) {
        let x = f.model_inputs();
        let scores: Vec<f64> = self
            .weights
            .iter()
            .zip(&self.bias)
            .map(|(row, b)| {
                b + row
                    .iter()
                    .enumerate()
                    .map(|(j, w)| w * (x[j] - self.mean[j]) / self.scale[j])
                    .sum::<f64>()
            })
            .collect();
        let top = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = scores.iter().map(|s| (s - top).exp()).collect();
        let total: f64 = exps.iter().sum();
        let (best, p) = exps
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e / total))
            .fold((0, 0.0), |acc, cur| if cur.1 > acc.1 { cur } else { acc });
        (self.classes[best], p)
    }
}

/// Either way of answering "what is this process?".
#[derive(Clone, Debug)]
pub enum Classifier {
    Trained(Model),
    Rules,
}

impl Classifier {
    pub fn classify(&self, f: &Features) -> (Class, f64) {
        match self {
            Classifier::Trained(m) => m.predict(f),
            Classifier::Rules => (rules(f), 1.0),
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Classifier::Trained(_) => "trained model",
            Classifier::Rules => "built-in rules",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(cpu: f64, vol: f64, invol: f64, slice: f64) -> Features {
        Features {
            cpu_util: cpu,
            vol_rate: vol,
            invol_rate: invol,
            slice_ms: slice,
            wait_ratio: 0.0,
        }
    }

    #[test]
    fn rules_tell_the_four_kinds_apart() {
        assert_eq!(rules(&f(0.0, 0.0, 0.0, 0.0)), Class::Background);
        assert_eq!(rules(&f(1.0, 1.0, 80.0, 90.0)), Class::Batch);
        assert_eq!(rules(&f(0.05, 120.0, 0.0, 0.3)), Class::Interactive);
        assert_eq!(rules(&f(0.4, 60.0, 5.0, 6.0)), Class::Media);
    }

    #[test]
    fn rules_see_a_compiler_on_a_crowded_machine() {
        // a third of a core, waiting twice as long as it runs, never blocking by itself
        let crowded = Features {
            wait_ratio: 2.0,
            ..f(0.33, 0.0, 170.0, 2.0)
        };
        assert_eq!(rules(&crowded), Class::Batch);
        // a frame loop that waits as long, but blocks 60 times a second, is not batch
        let frames = Features {
            wait_ratio: 2.0,
            ..f(0.33, 60.0, 30.0, 5.0)
        };
        assert_eq!(rules(&frames), Class::Media);
    }

    /// A model that scores "batch" by cpu_util and "background" by its absence.
    fn tiny() -> Model {
        Model {
            version: 1,
            classes: vec![Class::Batch, Class::Background],
            mean: vec![0.0; 5],
            scale: vec![1.0; 5],
            weights: vec![vec![8.0, 0.0, 0.0, 0.0, 0.0], vec![0.0; 5]],
            bias: vec![-4.0, 0.0],
        }
    }

    #[test]
    fn a_model_scores_and_reports_its_confidence() {
        let m = tiny();
        let (busy, p_busy) = m.predict(&f(1.0, 0.0, 0.0, 0.0));
        assert_eq!(busy, Class::Batch);
        assert!(p_busy > 0.95, "{p_busy}");
        let (idle, p_idle) = m.predict(&f(0.0, 0.0, 0.0, 0.0));
        assert_eq!(idle, Class::Background);
        assert!(p_idle > 0.95, "{p_idle}");
        let (_, unsure) = m.predict(&f(0.5, 0.0, 0.0, 0.0)); // exactly on the line
        assert!((unsure - 0.5).abs() < 1e-9);
    }

    #[test]
    fn huge_scores_do_not_overflow() {
        let mut m = tiny();
        m.weights[0][0] = 1e6;
        let (class, p) = m.predict(&f(1.0, 0.0, 0.0, 0.0));
        assert_eq!(class, Class::Batch);
        assert!(p.is_finite() && p <= 1.0);
    }

    #[test]
    fn a_model_round_trips_through_json() {
        let m = tiny();
        let text = serde_json::to_string(&m).unwrap();
        assert!(text.contains("\"batch\""), "{text}");
        let back = Model::from_json(&text).unwrap();
        assert_eq!(back.classes, m.classes);
        assert_eq!(back.predict(&f(1.0, 0.0, 0.0, 0.0)).0, Class::Batch);
    }

    #[test]
    fn a_model_that_does_not_fit_is_refused() {
        let good = serde_json::to_value(tiny()).unwrap();
        let broken = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut v = good.clone();
            edit(&mut v);
            Model::from_json(&v.to_string())
        };
        assert!(matches!(
            broken(&|v| v["mean"] = serde_json::json!([0.0])),
            Err(ModelError::Shape(_))
        ));
        assert!(matches!(
            broken(&|v| v["bias"] = serde_json::json!([0.0])),
            Err(ModelError::Shape(_))
        ));
        assert!(matches!(
            broken(&|v| v["scale"][2] = serde_json::json!(0.0)),
            Err(ModelError::Shape(_))
        ));
        assert!(matches!(
            broken(&|v| v["classes"] = serde_json::json!(["batch"])),
            Err(ModelError::Shape(_))
        ));
        assert!(matches!(
            Model::from_json("{not json"),
            Err(ModelError::Parse(_))
        ));
        assert!(matches!(
            Model::from_json(r#"{"version":1}"#),
            Err(ModelError::Parse(_))
        ));
    }

    #[test]
    fn the_classifier_uses_either_source() {
        let busy = f(1.0, 0.0, 0.0, 0.0);
        assert_eq!(Classifier::Trained(tiny()).classify(&busy).0, Class::Batch);
        assert_eq!(Classifier::Rules.classify(&busy), (Class::Batch, 1.0));
        assert_eq!(Class::Media.to_string(), "media");
    }
}
