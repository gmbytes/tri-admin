pub mod api_catalog;
pub mod api_tokens;
pub mod applications;
pub mod auth;
pub mod codegen;
pub mod data_tools;
pub mod dictionaries;
pub mod extensions;
pub mod files;
pub mod health;
pub mod logs;
pub mod mcp;
pub mod menus;
pub mod open_access;
pub mod parameters;
pub mod rbac;
pub mod roles;
pub mod settings;
pub mod users;
pub mod versions;

pub fn registry() -> Vec<crate::common::api::ApiModule> {
    use crate::common::api::ApiModule;
    vec![
        ApiModule {
            prefix: workflows::router::API_PREFIX,
            endpoints: workflows::router::API_ENDPOINTS,
            routes: workflows::router::configured_routes,
        },
        ApiModule {
            prefix: ai::router::API_PREFIX,
            endpoints: ai::router::API_ENDPOINTS,
            routes: ai::router::configured_routes,
        },
        ApiModule {
            prefix: mcp::router::API_PREFIX,
            endpoints: mcp::router::API_ENDPOINTS,
            routes: mcp::router::configured_routes,
        },
        ApiModule {
            prefix: extensions::router::API_PREFIX,
            endpoints: extensions::router::API_ENDPOINTS,
            routes: extensions::router::configured_routes,
        },
        ApiModule {
            prefix: codegen::router::API_PREFIX,
            endpoints: codegen::router::API_ENDPOINTS,
            routes: codegen::router::configured_routes,
        },
        ApiModule {
            prefix: open_access::router::API_PREFIX,
            endpoints: open_access::router::API_ENDPOINTS,
            routes: open_access::router::configured_routes,
        },
        ApiModule {
            prefix: applications::router::API_PREFIX,
            endpoints: applications::router::API_ENDPOINTS,
            routes: applications::router::configured_routes,
        },
        ApiModule {
            prefix: api_tokens::router::API_PREFIX,
            endpoints: api_tokens::router::API_ENDPOINTS,
            routes: api_tokens::router::configured_routes,
        },
        ApiModule {
            prefix: health::API_PREFIX,
            endpoints: health::API_ENDPOINTS,
            routes: health::routes,
        },
        ApiModule {
            prefix: auth::router::API_PREFIX,
            endpoints: auth::router::API_ENDPOINTS,
            routes: auth::router::routes,
        },
        ApiModule {
            prefix: users::router::API_PREFIX,
            endpoints: users::router::API_ENDPOINTS,
            routes: users::router::routes,
        },
        ApiModule {
            prefix: roles::router::API_PREFIX,
            endpoints: roles::router::API_ENDPOINTS,
            routes: roles::router::routes,
        },
        ApiModule {
            prefix: menus::router::API_PREFIX,
            endpoints: menus::router::API_ENDPOINTS,
            routes: menus::router::routes,
        },
        ApiModule {
            prefix: dictionaries::router::API_PREFIX,
            endpoints: dictionaries::router::API_ENDPOINTS,
            routes: dictionaries::router::routes,
        },
        ApiModule {
            prefix: parameters::router::API_PREFIX,
            endpoints: parameters::router::API_ENDPOINTS,
            routes: parameters::router::routes,
        },
        ApiModule {
            prefix: api_catalog::API_PREFIX,
            endpoints: api_catalog::API_ENDPOINTS,
            routes: api_catalog::routes,
        },
        ApiModule {
            prefix: versions::router::API_PREFIX,
            endpoints: versions::router::API_ENDPOINTS,
            routes: versions::router::configured_routes,
        },
        ApiModule {
            prefix: logs::router::API_PREFIX,
            endpoints: logs::router::API_ENDPOINTS,
            routes: logs::router::routes,
        },
        ApiModule {
            prefix: settings::router::API_PREFIX,
            endpoints: settings::router::API_ENDPOINTS,
            routes: settings::router::routes,
        },
        ApiModule {
            prefix: files::router::API_PREFIX,
            endpoints: files::router::API_ENDPOINTS,
            routes: files::router::routes,
        },
        ApiModule {
            prefix: data_tools::router::API_PREFIX,
            endpoints: data_tools::router::API_ENDPOINTS,
            routes: data_tools::router::configured_routes,
        },
    ]
}
pub mod ai;
pub mod workflows;
