use admin_api::modules::extensions::tool_binding::compile;
use serde_json::{Value, json};
use tri_extension_package::package::ToolBinding;

fn binding(operation: &str, schema: Value, mappings: Value) -> ToolBinding {
    serde_json::from_value(json!({"name":"alias","description":"Managed binding","operation":operation,"inputSchema":schema,"mappings":mappings})).unwrap()
}
fn create_binding() -> ToolBinding {
    binding(
        "parameters.create",
        json!({"type":"object","properties":{"label":{"type":"string"},"key":{"type":"string"},"kind":{"enum":["string","number","boolean","json"]},"value":{}},"required":["label","key","kind","value"],"additionalProperties":false}),
        json!({"name":"label","key":"key","valueType":"kind","value":"value"}),
    )
}

#[test]
fn declared_aliases_map_only_to_actual_operation_fields() {
    let compiled=compile(&binding("parameters.list",json!({"type":"object","properties":{"prefix":{"type":"string","maxLength":100}},"additionalProperties":false}),json!({"search":"prefix"}))).unwrap();
    assert_eq!(
        compiled.arguments(&json!({"prefix":"record"})).unwrap(),
        json!({"search":"record"})
    );
    assert!(compiled.arguments(&json!({"prefix":4})).is_err());
    assert!(
        compiled
            .arguments(&json!({"prefix":"record","actorId":"forged"}))
            .is_err()
    );
    assert_eq!(compiled.permission(), "parameters.read");
    assert!(compiled.read_only());
}
#[test]
fn flat_write_mapping_builds_the_native_input_without_changing_integer_values() {
    let compiled = compile(&create_binding()).unwrap();
    let args=compiled.arguments(&json!({"label":"Maximum","key":"maximum","kind":"json","value":18446744073709551615u64})).unwrap();
    assert_eq!(
        args,
        json!({"input":{"name":"Maximum","key":"maximum","valueType":"json","value":18446744073709551615u64}})
    );
    assert!(args.to_string().contains("18446744073709551615"));
    assert_eq!(compiled.permission(), "parameters.write");
    assert!(!compiled.read_only());
    assert!(
        compiled
            .arguments(&json!({"label":"bad","key":"bad","kind":"invented","value":0}))
            .is_err()
    );
    assert!(
        compiled
            .arguments(&json!({"label":"missing","kind":"json","value":0}))
            .is_err()
    );
}
#[test]
fn missing_required_or_unknown_target_and_unimplemented_operations_cannot_compile() {
    let mut value = create_binding();
    value.mappings.remove("key");
    assert!(compile(&value).is_err());
    let mut value = create_binding();
    value.mappings.insert("actorId".into(), "label".into());
    assert!(compile(&value).is_err());
    let mut value = create_binding();
    value.operation = "users.becomeOwner".into();
    assert!(compile(&value).is_err());
    let mut value = create_binding();
    value.mappings.insert("input.name".into(), "label".into());
    assert!(compile(&value).is_err());
}
#[test]
fn source_schema_and_native_schema_both_constrain_calls() {
    let compiled=compile(&binding("parameters.get",json!({"type":"object","properties":{"record":{"type":"string","pattern":"^[a-f0-9]+$"}},"required":["record"]}),json!({"id":"record"}))).unwrap();
    assert!(compiled.arguments(&json!({"record":"abc"})).is_err());
    assert!(
        compiled
            .arguments(&json!({"record":"Z".repeat(24)}))
            .is_err()
    );
    assert_eq!(
        compiled
            .arguments(&json!({"record":"a".repeat(24)}))
            .unwrap(),
        json!({"id":"a".repeat(24)})
    );
}
#[test]
fn schema_references_cannot_read_files_or_remote_resources() {
    for reference in [
        "file:///C:/synthetic-private-schema.json",
        "https://127.0.0.1:9/private-schema.json",
    ] {
        assert!(
            compile(&binding(
                "parameters.list",
                json!({"type":"object","properties":{"prefix":{"$ref":reference}}}),
                json!({"search":"prefix"})
            ))
            .is_err()
        );
    }
    assert!(compile(&binding("parameters.list",json!({"type":"object","$defs":{"prefix":{"type":"string"}},"properties":{"prefix":{"$ref":"#/$defs/prefix"}}}),json!({"search":"prefix"}))).is_ok());
    let path =
        std::env::temp_dir().join(format!("tri-plugin-schema-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&path, br#"{"type":"string"}"#).unwrap();
    let reference = url::Url::from_file_path(&path).unwrap().to_string();
    let result = compile(&binding(
        "parameters.list",
        json!({"type":"object","properties":{"prefix":{"$ref":reference}}}),
        json!({"search":"prefix"}),
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(
        result.is_err(),
        "Even an existing valid local schema must not be read"
    );
}

#[test]
fn schema_size_depth_and_backtracking_patterns_have_explicit_limits() {
    let oversized = json!({"type":"object","properties":{"prefix":{"type":"string","description":"x".repeat(65536)}}});
    assert!(
        compile(&binding(
            "parameters.list",
            oversized,
            json!({"search":"prefix"})
        ))
        .is_err()
    );
    let mut nested = json!({"type":"string"});
    for _ in 0..40 {
        nested = json!({"type":"object","properties":{"value":nested}});
    }
    assert!(
        compile(&binding(
            "parameters.list",
            json!({"type":"object","properties":{"prefix":nested}}),
            json!({"search":"prefix"})
        ))
        .is_err()
    );
    assert!(
        compile(&binding(
            "parameters.list",
            json!({"type":"object","properties":{"prefix":{"type":"string","pattern":"(a+)\\1"}}}),
            json!({"search":"prefix"})
        ))
        .is_err()
    );
}

#[test]
fn repeated_source_fields_cannot_amplify_the_native_argument_budget() {
    let compiled=compile(&binding("parameters.create",json!({"type":"object","properties":{"payload":{"type":"string"},"key":{"type":"string"},"kind":{"enum":["string"]}},"required":["payload","key","kind"],"additionalProperties":false}),json!({"name":"payload","key":"key","valueType":"kind","value":"payload"}))).unwrap();
    assert!(
        compiled
            .arguments(&json!({"payload":"x".repeat(40000),"key":"amplification","kind":"string"}))
            .is_err()
    );
}

#[test]
fn schema_compilation_does_not_connect_to_http_peers() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let reference = format!("http://{}/schema", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    let mut bytes = [0; 4096];
                    let _ = stream.read(&mut bytes);
                    let body = br#"{"type":"string"}"#;
                    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                    stream.write_all(body).unwrap();
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("fixture listener failed: {error}"),
            }
        }
        false
    });
    let result = compile(&binding(
        "parameters.list",
        json!({"type":"object","properties":{"prefix":{"$ref":reference}}}),
        json!({"search":"prefix"}),
    ));
    let contacted = server.join().unwrap();
    assert!(result.is_err());
    assert!(!contacted);
}

#[test]
fn update_and_child_query_keep_route_fields_outside_native_input_wrappers() {
    let update=compile(&binding("parameters.update",json!({"type":"object","properties":{"record":{"type":"string"},"revision":{"type":"integer"},"caption":{"type":"string"}},"required":["record","revision"]}),json!({"id":"record","version":"revision","name":"caption"}))).unwrap();
    assert_eq!(
        update
            .arguments(&json!({"record":"a".repeat(24),"revision":3,"caption":"Revised"}))
            .unwrap(),
        json!({"id":"a".repeat(24),"input":{"version":3,"name":"Revised"}})
    );
    let query=compile(&binding("dictionaries.items.list",json!({"type":"object","properties":{"dictionary":{"type":"string"},"index":{"type":"integer"},"prefix":{"type":"string"}},"required":["dictionary"]}),json!({"id":"dictionary","page":"index","search":"prefix"}))).unwrap();
    assert_eq!(
        query
            .arguments(&json!({"dictionary":"b".repeat(24),"index":2,"prefix":"entry"}))
            .unwrap(),
        json!({"id":"b".repeat(24),"query":{"page":2,"search":"entry"}})
    );
    assert!(
        query
            .arguments(&json!({"dictionary":"b".repeat(24),"index":0}))
            .is_err()
    );
}
