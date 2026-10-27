//! Explicit bindings to implemented admin operations. Package data cannot add handlers.
use crate::{
    common::{error::AppError, pagination::PageInput, validation},
    modules::{auth::AuthContext, codegen, dictionaries, parameters, rbac::shared::Actor},
    state::AppState,
};
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, CallToolResult, Tool, ToolAnnotations},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

struct Binding {
    name: &'static str,
    permission: &'static str,
    read_only: bool,
    replay: bool,
    description: &'static str,
    schema: Value,
}
fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn id() -> Value {
    json!({"type":"string","pattern":"^[0-9a-f]{24}$"})
}
fn text() -> Value {
    json!({"type":"string"})
}
fn version() -> Value {
    json!({"type":"integer","minimum":1,"maximum":9007199254740990i64})
}
fn uuid() -> Value {
    json!({"type":"string","format":"uuid"})
}
fn page() -> Value {
    object(
        json!({"page":{"type":"integer","minimum":1},"pageSize":{"type":"integer","minimum":1,"maximum":100},"search":text(),"sort":text()}),
        &[],
    )
}
fn schema() -> Value {
    let field = object(
        json!({"name":text(),"label":text(),"kind":{"enum":["text","integer","number","boolean","datetime","textList"]},"required":{"type":"boolean"},"searchable":{"type":"boolean"},"unique":{"type":"boolean"}}),
        &["name", "label", "kind"],
    );
    object(
        json!({"resource":text(),"label":text(),"fields":{"type":"array","minItems":1,"maxItems":40,"items":field}}),
        &["resource", "label", "fields"],
    )
}
fn bindings() -> Vec<Binding> {
    let mut rows = vec![];
    let mut add = |name, permission, read_only, replay, description, schema| {
        rows.push(Binding {
            name,
            permission,
            read_only,
            replay,
            description,
            schema,
        })
    };
    for (prefix, read, write, create, patch) in [
        (
            "parameters",
            "parameters.read",
            "parameters.write",
            object(
                json!({"name":text(),"key":text(),"group":text(),"description":text(),"valueType":{"enum":["string","number","boolean","json"]},"value":{}}),
                &["name", "key", "valueType", "value"],
            ),
            object(
                json!({"version":version(),"name":text(),"key":text(),"group":text(),"description":text(),"valueType":{"enum":["string","number","boolean","json"]},"value":{}}),
                &["version"],
            ),
        ),
        (
            "dictionaries",
            "dictionaries.read",
            "dictionaries.write",
            object(
                json!({"name":text(),"key":text(),"description":text(),"enabled":{"type":"boolean"}}),
                &["name", "key"],
            ),
            object(
                json!({"version":version(),"name":text(),"key":text(),"description":text(),"enabled":{"type":"boolean"}}),
                &["version"],
            ),
        ),
    ] {
        let (list, get, create_name, update, delete) = if prefix == "parameters" {
            (
                "parameters.list",
                "parameters.get",
                "parameters.create",
                "parameters.update",
                "parameters.delete",
            )
        } else {
            (
                "dictionaries.list",
                "dictionaries.get",
                "dictionaries.create",
                "dictionaries.update",
                "dictionaries.delete",
            )
        };
        add(list, read, true, true, "分页查询当前业务资料", page());
        add(
            get,
            read,
            true,
            true,
            "按标识读取业务资料",
            object(json!({"id":id()}), &["id"]),
        );
        add(
            create_name,
            write,
            false,
            false,
            "创建业务资料；响应丢失时先查询，不自动重发",
            object(json!({"input":create}), &["input"]),
        );
        add(
            update,
            write,
            false,
            false,
            "按当前版本修改业务资料；版本冲突时刷新",
            object(json!({"id":id(),"input":patch}), &["id", "input"]),
        );
        add(
            delete,
            write,
            false,
            false,
            "按当前版本删除业务资料",
            object(json!({"id":id(),"version":version()}), &["id", "version"]),
        );
    }
    add(
        "dictionaries.items.list",
        "dictionaries.read",
        true,
        true,
        "分页查询活动字典的明细",
        object(json!({"id":id(),"query":page()}), &["id"]),
    );
    add(
        "dictionaries.items.create",
        "dictionaries.write",
        false,
        false,
        "创建活动字典明细；不自动重发",
        object(
            json!({"id":id(),"input":object(json!({"label":text(),"value":text(),"sort":{"type":"integer"},"enabled":{"type":"boolean"}}),&["label","value"])}),
            &["id", "input"],
        ),
    );
    add(
        "dictionaries.items.update",
        "dictionaries.write",
        false,
        false,
        "按版本修改字典明细",
        object(
            json!({"id":id(),"itemId":id(),"input":object(json!({"version":version(),"label":text(),"value":text(),"sort":{"type":"integer"},"enabled":{"type":"boolean"}}),&["version"])}),
            &["id", "itemId", "input"],
        ),
    );
    add(
        "dictionaries.items.delete",
        "dictionaries.write",
        false,
        false,
        "按版本删除字典明细",
        object(
            json!({"id":id(),"itemId":id(),"version":version()}),
            &["id", "itemId", "version"],
        ),
    );
    add(
        "codegen.definitions.list",
        "codegen.read",
        true,
        true,
        "按当前数据范围分页查询资源定义",
        page(),
    );
    add(
        "codegen.definitions.get",
        "codegen.read",
        true,
        true,
        "按当前数据范围读取定义",
        object(json!({"id":id()}), &["id"]),
    );
    add(
        "codegen.definitions.byRequest",
        "codegen.read",
        true,
        true,
        "按本人原 UUID 查询定义回执",
        object(json!({"requestId":uuid()}), &["requestId"]),
    );
    add(
        "codegen.definitions.create",
        "codegen.write",
        false,
        true,
        "保存新资源定义；同一操作者 UUID 只产生一个结果",
        object(
            json!({"requestId":uuid(),"schema":schema()}),
            &["requestId", "schema"],
        ),
    );
    add(
        "codegen.definitions.update",
        "codegen.write",
        false,
        false,
        "按版本和当前数据范围修改资源定义",
        object(
            json!({"id":id(),"input":object(json!({"version":version(),"schema":schema()}),&["version","schema"])}),
            &["id", "input"],
        ),
    );
    add(
        "codegen.definitions.delete",
        "codegen.write",
        false,
        false,
        "按版本和当前数据范围删除定义",
        object(json!({"id":id(),"version":version()}), &["id", "version"]),
    );
    add(
        "codegen.generations.list",
        "codegen.read",
        true,
        true,
        "查询当前数据范围内的不可变生成记录",
        object(
            json!({"page":{"type":"integer","minimum":1},"pageSize":{"type":"integer","minimum":1,"maximum":100},"search":text(),"definitionId":id()}),
            &[],
        ),
    );
    add(
        "codegen.generations.get",
        "codegen.read",
        true,
        true,
        "读取原生成结果资料",
        object(json!({"id":id()}), &["id"]),
    );
    add(
        "codegen.generations.byRequest",
        "codegen.read",
        true,
        true,
        "按本人原 UUID 查询生成回执",
        object(json!({"requestId":uuid()}), &["requestId"]),
    );
    add(
        "codegen.generations.create",
        "codegen.write",
        false,
        true,
        "生成实际新 Rust/Vue 工程；同一 UUID 返回原结果，不重做生成",
        object(
            json!({"requestId":uuid(),"definitionId":id(),"definitionVersion":version()}),
            &["requestId", "definitionId", "definitionVersion"],
        ),
    );
    add(
        "codegen.generations.preview",
        "codegen.read",
        true,
        true,
        "预览原生成工程内的已登记文件",
        object(json!({"id":id(),"path":text()}), &["id", "path"]),
    );
    rows
}
pub(crate) struct Contract {
    pub permission: &'static str,
    pub read_only: bool,
    pub tool: Tool,
}
fn contract(row: Binding) -> Contract {
    let mut tool = Tool::new(
        row.name,
        row.description,
        row.schema.as_object().cloned().unwrap_or_default(),
    );
    tool.annotations = Some(
        ToolAnnotations::new()
            .read_only(row.read_only)
            .destructive(!row.read_only && !row.name.ends_with(".create"))
            .idempotent(row.replay)
            .open_world(false),
    );
    Contract {
        permission: row.permission,
        read_only: row.read_only,
        tool,
    }
}
pub(crate) fn contracts() -> Vec<Contract> {
    bindings().into_iter().map(contract).collect()
}
pub fn catalog(context: &AuthContext) -> Vec<Tool> {
    contracts()
        .into_iter()
        .filter(|row| {
            context.can(row.permission)
                && (!row.tool.name.starts_with("codegen.") || context.can("codegen.read"))
        })
        .map(|row| row.tool)
        .collect()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestId {
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input<T> {
    input: T,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update<T> {
    id: String,
    input: T,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    id: String,
    version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Items {
    id: String,
    query: Option<PageInput>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ItemUpdate<T> {
    id: String,
    item_id: String,
    input: T,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ItemDelete {
    id: String,
    item_id: String,
    version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    id: String,
    path: String,
}
fn parse<T: DeserializeOwned>(value: Value) -> Result<T, AppError> {
    serde_json::from_value(value).map_err(|_| AppError::Validation("MCP 工具参数无效".into()))
}
fn data<T: Serialize>(value: T) -> Result<Value, AppError> {
    Ok(json!({"data":serde_json::to_value(value).map_err(|_|AppError::Internal)?}))
}
fn page_data<T: Serialize>(value: T) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| AppError::Internal)
}
async fn execute(
    state: &AppState,
    context: &AuthContext,
    name: &str,
    args: Value,
) -> Result<Value, AppError> {
    let database = state.require_database()?;
    let actor = Actor::new(context)?;
    match name {
        "parameters.list" => page_data(parameters::service::list(database, parse(args)?).await?),
        "parameters.get" => {
            let v: Id = parse(args)?;
            data(parameters::service::get(database, validation::object_id(&v.id)?).await?)
        }
        "parameters.create" => {
            let v: Input<parameters::dto::CreateParameter> = parse(args)?;
            data(parameters::service::create(database, actor, v.input).await?)
        }
        "parameters.update" => {
            let v: Update<parameters::dto::PatchParameter> = parse(args)?;
            data(
                parameters::service::patch(database, actor, validation::object_id(&v.id)?, v.input)
                    .await?,
            )
        }
        "parameters.delete" => {
            let v: Delete = parse(args)?;
            parameters::service::delete(database, actor, validation::object_id(&v.id)?, v.version)
                .await?;
            data(Value::Null)
        }
        "dictionaries.list" => {
            page_data(dictionaries::service::list(database, parse(args)?).await?)
        }
        "dictionaries.get" => {
            let v: Id = parse(args)?;
            data(dictionaries::service::get(database, validation::object_id(&v.id)?).await?)
        }
        "dictionaries.create" => {
            let v: Input<dictionaries::dto::CreateDictionary> = parse(args)?;
            data(dictionaries::service::create(database, actor, v.input).await?)
        }
        "dictionaries.update" => {
            let v: Update<dictionaries::dto::PatchDictionary> = parse(args)?;
            data(
                dictionaries::service::patch(
                    database,
                    actor,
                    validation::object_id(&v.id)?,
                    v.input,
                )
                .await?,
            )
        }
        "dictionaries.delete" => {
            let v: Delete = parse(args)?;
            dictionaries::service::delete(
                database,
                actor,
                validation::object_id(&v.id)?,
                v.version,
            )
            .await?;
            data(Value::Null)
        }
        "dictionaries.items.list" => {
            let v: Items = parse(args)?;
            page_data(
                dictionaries::service::list_items(
                    database,
                    validation::object_id(&v.id)?,
                    v.query.unwrap_or(parse(json!({}))?),
                )
                .await?,
            )
        }
        "dictionaries.items.create" => {
            let v: Update<dictionaries::dto::CreateItem> = parse(args)?;
            data(
                dictionaries::service::create_item(
                    database,
                    actor,
                    validation::object_id(&v.id)?,
                    v.input,
                )
                .await?,
            )
        }
        "dictionaries.items.update" => {
            let v: ItemUpdate<dictionaries::dto::PatchItem> = parse(args)?;
            data(
                dictionaries::service::patch_item(
                    database,
                    actor,
                    validation::object_id(&v.id)?,
                    validation::object_id(&v.item_id)?,
                    v.input,
                )
                .await?,
            )
        }
        "dictionaries.items.delete" => {
            let v: ItemDelete = parse(args)?;
            dictionaries::service::delete_item(
                database,
                actor,
                validation::object_id(&v.id)?,
                validation::object_id(&v.item_id)?,
                v.version,
            )
            .await?;
            data(Value::Null)
        }
        "codegen.definitions.list" => {
            page_data(codegen::service::definitions(database, context, parse(args)?).await?)
        }
        "codegen.definitions.get" => {
            let v: Id = parse(args)?;
            data(
                codegen::service::definition(database, context, validation::object_id(&v.id)?)
                    .await?,
            )
        }
        "codegen.definitions.byRequest" => {
            let v: RequestId = parse(args)?;
            data(codegen::service::definition_by_request(database, context, &v.request_id).await?)
        }
        "codegen.definitions.create" => {
            let (row, created) =
                codegen::service::create_definition(database, context, parse(args)?).await?;
            Ok(json!({"data":row,"created":created}))
        }
        "codegen.definitions.update" => {
            let v: Update<codegen::dto::UpdateDefinition> = parse(args)?;
            data(
                codegen::service::update_definition(
                    database,
                    context,
                    validation::object_id(&v.id)?,
                    v.input,
                )
                .await?,
            )
        }
        "codegen.definitions.delete" => {
            let v: Delete = parse(args)?;
            codegen::service::delete_definition(
                database,
                context,
                validation::object_id(&v.id)?,
                v.version,
            )
            .await?;
            data(Value::Null)
        }
        "codegen.generations.list" => {
            page_data(codegen::service::generations(database, context, parse(args)?).await?)
        }
        "codegen.generations.get" => {
            let v: Id = parse(args)?;
            data(
                codegen::service::generation(database, context, validation::object_id(&v.id)?)
                    .await?,
            )
        }
        "codegen.generations.byRequest" => {
            let v: RequestId = parse(args)?;
            data(codegen::service::generation_by_request(database, context, &v.request_id).await?)
        }
        "codegen.generations.create" => {
            let (row, created) =
                codegen::service::generate(database, context, parse(args)?).await?;
            Ok(json!({"data":row,"created":created}))
        }
        "codegen.generations.preview" => {
            let v: Preview = parse(args)?;
            data(
                codegen::service::preview(
                    database,
                    context,
                    validation::object_id(&v.id)?,
                    &v.path,
                )
                .await?,
            )
        }
        _ => Err(AppError::NotFound),
    }
}
pub async fn call(
    state: &AppState,
    context: &AuthContext,
    request: CallToolRequestParams,
) -> Result<CallToolResult, ErrorData> {
    let Some(binding) = bindings().into_iter().find(|row| row.name == request.name) else {
        return Err(ErrorData::invalid_params("工具不存在", None));
    };
    let permitted = context
        .require("mcp.invoke")
        .and_then(|_| context.require(binding.permission))
        .and_then(|_| {
            if binding.name.starts_with("codegen.") {
                context.require("codegen.read")
            } else {
                Ok(())
            }
        });
    if let Err(error) = permitted {
        return Ok(error_result(error, !binding.read_only));
    }
    if request.input_responses.is_some() || request.request_state.is_some() {
        return Err(ErrorData::invalid_params("此工具不接受多轮请求续传", None));
    }
    let arguments = Value::Object(request.arguments.unwrap_or_default());
    if arguments.to_string().len() > 64 * 1024 {
        return Err(ErrorData::invalid_params("工具参数超过限制", None));
    }
    match execute(state, context, binding.name, arguments).await {
        Ok(value) => {
            let result = CallToolResult::structured(value);
            if serde_json::to_vec(&result).map_or(true, |v| v.len() > 512 * 1024) {
                Ok(CallToolResult::structured_error(
                    json!({"error":{"code":"RESULT_LIMIT","message":"结果超过 MCP 预算，请缩小分页或改用文件接口","effect":if binding.read_only{"none"}else{"committed"}}}),
                ))
            } else {
                Ok(result)
            }
        }
        Err(error) => Ok(error_result(error, !binding.read_only)),
    }
}
pub(crate) fn error_result(error: AppError, write: bool) -> CallToolResult {
    let (code, message, unknown) = match error {
        AppError::Forbidden => ("FORBIDDEN", "当前账号权限或数据范围不足", false),
        AppError::Unauthorized => ("UNAUTHENTICATED", "当前凭据已失效", false),
        AppError::NotFound => ("NOT_FOUND", "资源不存在或不在当前范围", false),
        AppError::Validation(_) | AppError::BadRequest => ("INVALID_INPUT", "工具参数无效", false),
        AppError::Conflict(_) => ("CONFLICT", "当前版本或唯一性冲突，请先查询现有资料", false),
        AppError::Database(_) => (
            "DATABASE_UNAVAILABLE",
            "数据库暂不可用；修改结果需按原请求查询确认",
            write,
        ),
        _ => (
            "SERVICE_UNAVAILABLE",
            "业务服务暂不可用，请先查询原操作结果",
            write,
        ),
    };
    CallToolResult::structured_error(
        json!({"error":{"code":code,"message":message,"effect":if unknown{"unknown"}else{"none"}}}),
    )
}
