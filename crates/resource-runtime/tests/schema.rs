use serde_json::{Value, json};
use tri_resource_runtime::schema::Schema;

fn schema() -> Value {
    json!({"resource":"inventory_items","label":"库存","fields":[
        {"name":"sku","label":"编号","kind":"text","required":true,"searchable":true,"unique":true},
        {"name":"count","label":"数量","kind":"integer","required":true},
        {"name":"ratio","label":"比例","kind":"number","required":false},
        {"name":"active","label":"启用","kind":"boolean","required":true},
        {"name":"releasedAt","label":"时间","kind":"datetime","required":false},
        {"name":"tags","label":"标签","kind":"textList","required":false}
    ]})
}
#[test]
fn descriptor_is_exact_bounded_and_cannot_inject_names_or_metadata() {
    let parsed = Schema::parse(schema()).unwrap();
    assert_eq!(parsed.resource, "inventory_items");
    for resource in ["../outside", "users", "_schema", "x;panic!()", "BadCase"] {
        let mut value = schema();
        value["resource"] = json!(resource);
        assert!(Schema::parse(value).is_err());
    }
    for name in [
        "id",
        "ownerId",
        "version",
        "$where",
        "a.b",
        "snake_case",
        "123",
    ] {
        let mut value = schema();
        value["fields"][0]["name"] = json!(name);
        assert!(Schema::parse(value).is_err());
    }
    let mut duplicate = schema();
    duplicate["fields"][1]["name"] = json!("sku");
    assert!(Schema::parse(duplicate).is_err());
    let mut unknown = schema();
    unknown["sql"] = json!("ignored must not be accepted");
    assert!(Schema::parse(unknown).is_err());
    let mut invalid = schema();
    invalid["fields"][1]["searchable"] = json!(true);
    assert!(Schema::parse(invalid).is_err());
}
#[test]
fn typed_values_round_trip_without_extended_json_or_implicit_coercions() {
    let schema = Schema::parse(schema()).unwrap();
    let values = json!({"sku":"A-1","count":7,"ratio":1.25,"active":true,"releasedAt":"2026-10-26T08:00:00+08:00","tags":["a","b"]});
    let doc = schema.values(&values).unwrap();
    let view = schema.public_values(&doc).unwrap();
    assert_eq!(view["count"], 7);
    assert_eq!(view["releasedAt"], "2026-10-26T00:00:00.000Z");
    assert!(!view.to_string().contains("$date"));
    for (field, value) in [
        ("count", json!("7")),
        ("count", json!(1.5)),
        ("count", json!(9007199254740992u64)),
        ("ratio", json!(1e100)),
        ("active", json!("true")),
        ("releasedAt", json!("yesterday")),
        ("tags", json!(["a", 7])),
        ("sku", Value::Null),
        ("ownerId", json!("injected")),
    ] {
        let mut bad = values.clone();
        bad[field] = value;
        assert!(schema.values(&bad).is_err(), "{field}");
    }
    let mut missing = values.clone();
    missing.as_object_mut().unwrap().remove("count");
    assert!(schema.values(&missing).is_err());
    let minimal = schema
        .values(&json!({"sku":"A-2","count":0,"active":false}))
        .unwrap();
    assert_eq!(schema.public_values(&minimal).unwrap()["tags"], Value::Null);
    let mut wrong_storage = minimal.clone();
    wrong_storage.insert("sku", mongodb::bson::DateTime::now());
    assert!(schema.public_values(&wrong_storage).is_err());
    let mut wrong_date = minimal;
    wrong_date.insert("releasedAt", "2026-10-26T00:00:00.000Z");
    assert!(schema.public_values(&wrong_date).is_err());
}
#[test]
fn aggregate_record_size_keeps_maximum_pages_within_client_response_limit() {
    let fields = (0..20)
        .map(|index| json!({"name":format!("field{index}"),"label":"字段","kind":"text"}))
        .collect::<Vec<_>>();
    let schema =
        Schema::parse(json!({"resource":"bounded","label":"有界记录","fields":fields})).unwrap();
    let values = (0..20)
        .map(|index| (format!("field{index}"), json!("x".repeat(4096))))
        .collect::<serde_json::Map<_, _>>();
    assert!(schema.values(&Value::Object(values)).is_err());
}
