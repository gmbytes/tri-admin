use admin_api::{build_router, config::Config, state::AppState};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
async fn browser_mcp_preflight_allows_modern_headers_only_for_configured_origins() {
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some("fixture-mcp-preflight-signing-key-32bytes".into()),
        "TRI_CORS_ORIGINS" => Some("https://admin.example.test".into()),
        _ => None,
    })
    .unwrap();
    let router = build_router(AppState::new(config, None));
    for (origin, expected) in [
        ("https://admin.example.test", true),
        ("https://untrusted.example.test", false),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/v1/mcp/server")
                    .header("origin", origin)
                    .header("access-control-request-method", "POST")
                    .header(
                        "access-control-request-headers",
                        "authorization,content-type,mcp-protocol-version,mcp-method,mcp-name",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        if !expected {
            assert!(
                !response
                    .headers()
                    .contains_key("access-control-allow-origin")
            );
            continue;
        }
        assert_eq!(response.headers()["access-control-allow-origin"], origin);
        let allowed = response.headers()["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .split(',')
            .map(str::trim)
            .collect::<Vec<_>>();
        for header in [
            "authorization",
            "content-type",
            "mcp-protocol-version",
            "mcp-method",
            "mcp-name",
        ] {
            assert!(allowed.contains(&header), "{header}");
        }
        assert!(!allowed.contains(&"*"));
        assert!(!allowed.contains(&"x-unapproved-header"));
    }
}
