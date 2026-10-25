pub mod api_catalog;
pub mod auth;
pub mod dictionaries;
pub mod health;
pub mod menus;
pub mod parameters;
pub mod rbac;
pub mod roles;
pub mod users;
pub mod versions;

pub fn registry() -> Vec<crate::common::api::ApiModule> {
    use crate::common::api::ApiModule;
    vec![
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
            routes: versions::router::routes,
        },
    ]
}
