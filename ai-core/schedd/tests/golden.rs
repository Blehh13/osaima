//! The shipped model gives the same answers in Rust as it did in Python when it was trained.
//! `model/golden.json` holds measurements with scikit-learn's verdicts (written by
//! `train/train.py`); a mismatch means the two sides disagree about the maths.

use osaima_schedd::features::Features;
use osaima_schedd::model::{rules, Class, Model};
use serde::Deserialize;

#[derive(Deserialize)]
struct Example {
    features: Features,
    class: Class,
    probability: f64,
    /// What train.py's copy of the built-in rules said (older files do not have it).
    #[serde(default)]
    rules: Option<Class>,
}

fn load() -> (Model, Vec<Example>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/model");
    let model = std::fs::read_to_string(format!("{dir}/model.json")).expect("model/model.json");
    let golden = std::fs::read_to_string(format!("{dir}/golden.json")).expect("model/golden.json");
    (
        Model::from_json(&model).expect("the shipped model is valid"),
        serde_json::from_str(&golden).expect("model/golden.json is valid"),
    )
}

#[test]
fn rust_agrees_with_the_trained_model() {
    let (model, examples) = load();
    assert!(!examples.is_empty());
    for (i, ex) in examples.iter().enumerate() {
        let (class, p) = model.predict(&ex.features);
        assert_eq!(class, ex.class, "example {i}: {:?}", ex.features);
        // model.json rounds the weights to 10 digits, so allow a little drift
        assert!(
            (p - ex.probability).abs() < 1e-6,
            "example {i}: probability {p}, trained model said {}",
            ex.probability
        );
    }
}

#[test]
fn rust_and_python_rules_agree() {
    // the report compares the model with the rules, so both copies must be the same rules
    let (_, examples) = load();
    for (i, ex) in examples.iter().enumerate() {
        if let Some(expected) = ex.rules {
            assert_eq!(
                rules(&ex.features),
                expected,
                "example {i}: {:?}",
                ex.features
            );
        }
    }
}

#[test]
fn every_class_appears_in_the_examples() {
    let (_, examples) = load();
    for class in Class::ALL {
        assert!(
            examples.iter().any(|ex| ex.class == class),
            "no example of {class}"
        );
    }
}
