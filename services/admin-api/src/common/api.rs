use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    Public,
    Authenticated,
    RefreshCredential,
    Permission,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub operation_id: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub access: Access,
    pub required_permissions: &'static [&'static str],
    pub notes: &'static str,
}
pub struct ApiModule {
    pub prefix: &'static str,
    pub endpoints: &'static [Endpoint],
    pub routes: fn() -> axum::Router<crate::state::AppState>,
}

// Each declaration registers the live route and its metadata together.
#[macro_export]
macro_rules! api_routes {
    (@method get)=>{"GET"};
    (@method post)=>{"POST"};
    (@method patch)=>{"PATCH"};
    (@method delete)=>{"DELETE"};
    (prefix $prefix:literal; $( $method:ident $path:literal => $handler:ident, $operation:literal, $group:literal, $label:literal, $access:ident, $permissions:expr $(, $notes:literal)?; )*) => {
        pub const API_PREFIX:&str=$prefix;
        pub const API_ENDPOINTS:&[$crate::common::api::Endpoint]=&[$(
            $crate::common::api::Endpoint{operation_id:$operation,method:$crate::api_routes!(@method $method),path:concat!($prefix,$path),group:$group,label:$label,access:$crate::common::api::Access::$access,required_permissions:$permissions,notes:concat!($($notes)?)},
        )*];
        pub fn routes()->axum::Router<$crate::state::AppState> {
            let router=axum::Router::new();
            $(let router=router.route($path,axum::routing::$method($handler));)*
            router
        }
    };
}
