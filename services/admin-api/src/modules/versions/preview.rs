use super::{
    import_model::*,
    import_state,
    import_validation::{self, Validated},
    models::{Bundle, Dictionary, time},
};
use crate::{
    common::{
        canonical,
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::{
        menus,
        rbac::shared::{self, Actor},
    },
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;

fn fields<T: Serialize>(old: Option<&T>, desired: &T) -> Result<Vec<String>, AppError> {
    let desired = serde_json::to_value(desired).map_err(|_| AppError::Internal)?;
    let old = old
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| AppError::Internal)?;
    Ok(desired
        .as_object()
        .ok_or(AppError::Internal)?
        .iter()
        .filter(|(key, value)| old.as_ref().and_then(|old| old.get(*key)) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect())
}
struct Builder {
    counts: ImportCounts,
    changes: Vec<Change>,
    total: usize,
}
impl Builder {
    fn add<T: Serialize>(
        &mut self,
        resource: &str,
        key: &str,
        old: Option<&T>,
        desired: &T,
        strategy: Strategy,
    ) -> Result<(), AppError> {
        let fields = fields(old, desired)?;
        let action = if old.is_none() {
            "create"
        } else if fields.is_empty() {
            "unchanged"
        } else if strategy == Strategy::CreateOnly {
            "conflict"
        } else {
            "update"
        };
        let count = match resource {
            "menus" => &mut self.counts.menus,
            "dictionaries" => &mut self.counts.dictionaries,
            "dictionaryItems" => &mut self.counts.dictionary_items,
            _ => &mut self.counts.parameters,
        };
        match action {
            "create" => count.create += 1,
            "update" => count.update += 1,
            "conflict" => count.conflict += 1,
            _ => count.unchanged += 1,
        };
        self.total += 1;
        if self.changes.len() < 100 {
            self.changes.push(Change {
                resource: resource.into(),
                key: key.into(),
                action: action.into(),
                fields,
            });
        }
        Ok(())
    }
}
fn dictionary_meta(value: &Dictionary) -> Value {
    json!({"name":value.name,"key":value.key,"description":value.description,"enabled":value.enabled})
}
pub fn proposed_menu_ids(
    bundle: &Bundle,
    state: &TargetState,
) -> Result<HashMap<String, ObjectId>, AppError> {
    let mut ids = state
        .menus
        .iter()
        .map(|item| (item.config.path.clone(), item.id))
        .collect::<HashMap<_, _>>();
    for desired in &bundle.resources.menus {
        ids.entry(desired.path.clone())
            .or_insert_with(ObjectId::new);
    }
    let mut parents = HashMap::new();
    for item in &state.menus {
        let parent = item
            .config
            .parent_path
            .as_ref()
            .map(|path| {
                ids.get(path)
                    .copied()
                    .ok_or_else(|| AppError::Validation("目标菜单父级缺失".into()))
            })
            .transpose()?;
        parents.insert(item.id, parent);
    }
    for item in &bundle.resources.menus {
        let parent = item
            .parent_path
            .as_ref()
            .map(|path| {
                ids.get(path)
                    .copied()
                    .ok_or_else(|| AppError::Validation("包中父菜单不存在".into()))
            })
            .transpose()?;
        parents.insert(ids[&item.path], parent);
    }
    menus::service::validate_graph(&parents)
        .map_err(|_| AppError::Validation("导入后的菜单图存在循环、无效引用或层级超限".into()))?;
    Ok(ids)
}
pub fn plan(
    actor: &Actor,
    validated: &Validated,
    state: &TargetState,
    strategy: Strategy,
) -> Result<Plan, AppError> {
    proposed_menu_ids(&validated.bundle, state)?;
    let mut builder = Builder {
        counts: ImportCounts::default(),
        changes: vec![],
        total: 0,
    };
    for item in &validated.bundle.resources.menus {
        builder.add(
            "menus",
            &item.path,
            state
                .menus
                .iter()
                .find(|old| old.config.path == item.path)
                .map(|old| &old.config),
            item,
            strategy,
        )?;
    }
    for item in &validated.bundle.resources.dictionaries {
        let old = state
            .dictionaries
            .iter()
            .find(|old| old.config.key == item.key);
        let old_meta = old.map(|old| dictionary_meta(&old.config));
        let desired = dictionary_meta(item);
        builder.add(
            "dictionaries",
            &item.key,
            old_meta.as_ref(),
            &desired,
            strategy,
        )?;
        for entry in &item.items {
            let previous = old
                .and_then(|old| old.items.iter().find(|old| old.config.value == entry.value))
                .map(|old| &old.config);
            builder.add(
                "dictionaryItems",
                &format!("{}/{}", item.key, entry.value),
                previous,
                entry,
                strategy,
            )?;
        }
    }
    for item in &validated.bundle.resources.parameters {
        builder.add(
            "parameters",
            &item.key,
            state
                .parameters
                .iter()
                .find(|old| old.config.key == item.key)
                .map(|old| &old.config),
            item,
            strategy,
        )?;
    }
    let version_action = match &state.version {
        None => "create",
        Some(version)
            if !version.deleted
                && version.checksum == validated.checksum
                && version.payload_valid =>
        {
            "reuse"
        }
        _ => "conflict",
    }
    .to_owned();
    let missing = validated
        .writes
        .iter()
        .filter(|code| !actor.can(code))
        .cloned()
        .collect::<Vec<_>>();
    let conflicts = builder.counts.menus.conflict
        + builder.counts.dictionaries.conflict
        + builder.counts.dictionary_items.conflict
        + builder.counts.parameters.conflict;
    Ok(Plan {
        counts: builder.counts,
        changes: builder.changes,
        details_truncated: builder.total > 100,
        can_apply: conflicts == 0 && version_action != "conflict" && missing.is_empty(),
        version_action,
        missing_write_permissions: missing,
    })
}
pub async fn create(
    database: &Database,
    actor: Actor,
    input: PreviewInput,
) -> Result<PreviewView, AppError> {
    let validated = import_validation::validate(&actor, input.bundle)?;
    let id = ObjectId::new();
    transaction::run_snapshot(database,(database.clone(),actor,validated),|session,(database,actor,validated)|{
        async move {
            shared::lock(database,"versions",session).await?;
            let state=import_state::load(database,&validated.bundle,&validated.checksum,session).await?;
            let plan=plan(actor,validated,&state,input.strategy).map_err(|_|fail(Failure::Validation("无法生成导入计划，菜单图或目标配置无效")))?;
            let baseline=import_state::fingerprint(&state).map_err(|_|fail(Failure::Validation("目标配置无法编码")))?;
            let now=DateTime::now();let expires=DateTime::from_millis(now.timestamp_millis()+10*60*1000);
            let plan_json=canonical::json(&plan).map_err(|_|fail(Failure::Validation("导入计划无法编码")))?;
            database.collection::<Document>("version_previews").insert_one(doc!{"_id":id,"actorId":actor.id,"checksum":&validated.checksum,"baselineHash":baseline,"bundleJson":&validated.json,"strategy":input.strategy.as_str(),"readPermissions":&validated.reads,"writePermissions":&validated.writes,"planJson":plan_json,"status":"pending","createdAt":now,"expiresAt":expires,"cleanupAt":expires}).session(session).await?;
            Ok(PreviewView{id:id.to_hex(),checksum:validated.checksum.clone(),expires_at:time(expires).map_err(|_|fail(Failure::Validation("时间无法编码")))?,strategy:input.strategy,plan})
        }.boxed()
    }).await
}
