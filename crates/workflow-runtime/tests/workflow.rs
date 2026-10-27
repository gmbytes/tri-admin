use serde_json::json;
use std::collections::BTreeMap;
use tri_workflow_runtime::{
    Analysis, Entry, Error, Expected, Inputs, Mode, Outcome, Plan, Step, Transcript,
};
fn step(id: &str, depends: &[&str]) -> Step {
    Step {
        id: id.into(),
        title: id.into(),
        prompt: format!("perform {id}"),
        depends_on: depends.iter().map(|value| (*value).into()).collect(),
        expected: Expected::Text,
    }
}
fn plan() -> Plan {
    Plan {
        mode: Mode::Project,
        steps: vec![
            step("design", &["analysis"]),
            Step {
                expected: Expected::Analysis,
                ..step("analysis", &[])
            },
            step("deliver", &["design"]),
        ],
    }
}
fn inputs() -> Inputs {
    Inputs {
        requirements: "PRIVATE-REQUIREMENTS".into(),
        scene: "game operations".into(),
        constraints: "Rust/MongoDB/Vue".into(),
    }
}
#[test]
fn dependency_order_is_deterministic_and_never_skips_an_unfinished_dependency() {
    let plan = plan();
    assert_eq!(plan.order().unwrap(), vec![1, 0, 2]);
    assert_eq!(plan.next(&[]).unwrap().unwrap().id, "analysis");
    assert_eq!(
        plan.next(&["analysis".into()]).unwrap().unwrap().id,
        "design"
    );
    assert_eq!(
        plan.next(&["analysis".into(), "design".into()])
            .unwrap()
            .unwrap()
            .id,
        "deliver"
    );
    assert!(
        plan.next(&["analysis".into(), "design".into(), "deliver".into()])
            .unwrap()
            .is_none()
    );
}
#[test]
fn duplicate_unknown_self_and_cyclic_dependencies_are_rejected() {
    for steps in [
        vec![step("same", &[]), step("same", &[])],
        vec![step("a", &["missing"])],
        vec![step("a", &["a"])],
        vec![step("a", &["b"]), step("b", &["a"])],
        vec![step("a", &[]), step("b", &["a", "a"])],
    ] {
        assert!(
            Plan {
                mode: Mode::Feature,
                steps
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        Plan {
            mode: Mode::Project,
            steps: (0..17)
                .map(|index| step(&format!("s{index}"), &[]))
                .collect()
        }
        .validate()
        .is_err()
    );
}
#[test]
fn builtins_produce_editable_valid_plans_for_each_supported_goal() {
    for mode in [Mode::Project, Mode::Feature, Mode::McpGuide] {
        let plan = Plan::template(mode);
        plan.validate().unwrap();
        assert_eq!(plan.mode, mode);
        assert_eq!(
            plan.steps[plan.order().unwrap()[0]].expected,
            Expected::Analysis
        );
        assert!(plan.steps.len() >= 3);
        let encoded = serde_json::to_vec(&plan).unwrap();
        let restored: Plan = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored.order().unwrap(), plan.order().unwrap());
    }
}
#[test]
fn prompt_assembly_only_includes_declared_dependencies_and_preserves_private_input() {
    let plan = plan();
    let mut results = BTreeMap::new();
    results.insert("analysis".into(), "ANALYSIS-RESULT".into());
    results.insert("unrelated".into(), "FOREIGN-PRIVATE-RESULT".into());
    let prompt = plan.render("design", &inputs(), &results).unwrap();
    assert!(prompt.contains("PRIVATE-REQUIREMENTS"));
    assert!(prompt.contains("ANALYSIS-RESULT"));
    assert!(!prompt.contains("FOREIGN-PRIVATE-RESULT"));
    assert!(plan.render("deliver", &inputs(), &results).is_err());
    let error = plan
        .render("unknown-private-id", &inputs(), &results)
        .err()
        .unwrap();
    assert!(!format!("{error:?} {error}").contains("private-id"));
}
#[test]
fn assembled_prompts_are_bounded_without_truncation() {
    let plan = plan();
    let mut results = BTreeMap::new();
    results.insert("analysis".into(), "x".repeat(65536));
    assert!(matches!(
        plan.render("design", &inputs(), &results),
        Err(Error::Limit)
    ));
    let mut input = inputs();
    input.requirements = " ".into();
    assert!(plan.render("analysis", &input, &BTreeMap::new()).is_err());
}
fn analysis() -> serde_json::Value {
    json!({"summary":"Manage game worlds","modules":[{"key":"worlds","label":"Worlds","description":"World configuration","fields":[{"name":"name","label":"Name","kind":"string","required":true}]}],"risks":["Confirm role boundaries"],"questions":["Which regions?"]})
}
#[test]
fn strict_analysis_shape_and_local_semantics_match_the_provider_schema() {
    let value = analysis();
    let schema = Analysis::schema();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&value));
    let parsed = Analysis::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(parsed.modules[0].key, "worlds");
    let mut invalid = value.clone();
    invalid
        .as_object_mut()
        .unwrap()
        .insert("execute".into(), json!({"shell":"PRIVATE-COMMAND"}));
    assert!(!validator.is_valid(&invalid));
    let error = Analysis::parse(&serde_json::to_vec(&invalid).unwrap())
        .err()
        .unwrap();
    assert!(!format!("{error:?} {error}").contains("PRIVATE-COMMAND"));
}
#[test]
fn analysis_rejects_duplicate_modules_fields_invalid_types_and_unbounded_results() {
    let value = analysis();
    let mut duplicate = value.clone();
    duplicate["modules"]
        .as_array_mut()
        .unwrap()
        .push(value["modules"][0].clone());
    assert!(Analysis::parse(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    let mut fields = value.clone();
    fields["modules"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(value["modules"][0]["fields"][0].clone());
    assert!(Analysis::parse(&serde_json::to_vec(&fields).unwrap()).is_err());
    let mut invalid = value;
    invalid["modules"][0]["fields"][0]["kind"] = json!("shell");
    assert!(Analysis::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
    assert!(matches!(
        Analysis::parse(&vec![b'x'; 65537]),
        Err(Error::Limit)
    ));
}
#[test]
fn markdown_exports_preserve_content_inside_unbreakable_text_fences_and_match_the_manifest() {
    let transcript = Transcript {
        title: "Private <script> title".into(),
        outcome: Outcome::Unknown,
        entries: vec![Entry {
            title: "analysis".into(),
            outcome: Outcome::Failed,
            prompt: "PRIVATE-PROMPT".into(),
            text: "```\n# injected heading\n`````\n<script>private()</script>".into(),
        }],
    };
    let artifact = transcript.markdown().unwrap();
    let text = std::str::from_utf8(&artifact.bytes).unwrap();
    assert!(text.contains("PRIVATE-PROMPT"));
    assert!(text.contains("``````text\n```\n# injected heading"));
    assert!(text.contains("状态：unknown"));
    assert_eq!(artifact.size, artifact.bytes.len());
    use sha2::{Digest, Sha256};
    assert_eq!(
        artifact.sha256,
        format!("{:x}", Sha256::digest(&artifact.bytes))
    );
}
#[test]
fn markdown_export_budget_fails_instead_of_returning_a_partial_artifact() {
    let entry = Entry {
        title: "x".into(),
        outcome: Outcome::Succeeded,
        prompt: "p".into(),
        text: "x".repeat(1024 * 1024),
    };
    let transcript = Transcript {
        title: "bounded".into(),
        outcome: Outcome::Succeeded,
        entries: vec![entry; 5],
    };
    assert!(matches!(transcript.markdown(), Err(Error::Limit)));
}
