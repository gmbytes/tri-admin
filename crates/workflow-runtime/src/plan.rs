use crate::{Error, label};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Project,
    Feature,
    McpGuide,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Expected {
    Text,
    Analysis,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub depends_on: Vec<String>,
    pub expected: Expected,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    pub mode: Mode,
    pub steps: Vec<Step>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inputs {
    pub requirements: String,
    pub scene: String,
    pub constraints: String,
}
impl Inputs {
    pub fn validate(&self) -> Result<(), Error> {
        if self.requirements.trim().is_empty() {
            return Err(Error::Input);
        }
        if self.requirements.len() > 16384
            || self.scene.len() > 4096
            || self.constraints.len() > 8192
        {
            return Err(Error::Limit);
        }
        Ok(())
    }
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}
impl Plan {
    fn shape(&self) -> Result<(), Error> {
        if self.steps.is_empty() || self.steps.len() > 16 {
            return Err(Error::Plan);
        }
        let mut ids = HashSet::new();
        for step in &self.steps {
            if !id(&step.id)
                || !ids.insert(step.id.as_str())
                || !label(&step.title, 80)
                || step.prompt.trim().is_empty()
                || step.prompt.len() > 16384
                || step.depends_on.len() > 16
            {
                return Err(Error::Plan);
            }
        }
        for step in &self.steps {
            let mut unique = HashSet::new();
            for dependency in &step.depends_on {
                if dependency == &step.id
                    || !ids.contains(dependency.as_str())
                    || !unique.insert(dependency)
                {
                    return Err(Error::Plan);
                }
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), Error> {
        self.order().map(|_| ())
    }
    pub fn order(&self) -> Result<Vec<usize>, Error> {
        self.shape()?;
        let mut done = HashSet::new();
        let mut result = Vec::with_capacity(self.steps.len());
        while result.len() < self.steps.len() {
            let Some((index, step)) = self.steps.iter().enumerate().find(|(_, step)| {
                !done.contains(step.id.as_str())
                    && step.depends_on.iter().all(|id| done.contains(id.as_str()))
            }) else {
                return Err(Error::Plan);
            };
            done.insert(step.id.as_str());
            result.push(index);
        }
        Ok(result)
    }
    pub fn next(&self, completed: &[String]) -> Result<Option<&Step>, Error> {
        let order = self.order()?;
        let done = completed.iter().map(String::as_str).collect::<HashSet<_>>();
        if done.len() != completed.len()
            || done
                .iter()
                .any(|id| !self.steps.iter().any(|step| &step.id == id))
        {
            return Err(Error::Plan);
        }
        for step in &self.steps {
            if done.contains(step.id.as_str())
                && !step.depends_on.iter().all(|id| done.contains(id.as_str()))
            {
                return Err(Error::Plan);
            }
        }
        Ok(order
            .into_iter()
            .map(|index| &self.steps[index])
            .find(|step| {
                !done.contains(step.id.as_str())
                    && step.depends_on.iter().all(|id| done.contains(id.as_str()))
            }))
    }
    pub fn render(
        &self,
        step_id: &str,
        inputs: &Inputs,
        results: &BTreeMap<String, String>,
    ) -> Result<String, Error> {
        self.validate()?;
        inputs.validate()?;
        let step = self
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .ok_or(Error::Plan)?;
        let mut result = String::new();
        let mut push = |value: &str| -> Result<(), Error> {
            if result
                .len()
                .checked_add(value.len())
                .is_none_or(|size| size > 65536)
            {
                return Err(Error::Limit);
            }
            result.push_str(value);
            Ok(())
        };
        push("任务说明：\n")?;
        push(&step.prompt)?;
        push("\n需求输入（以下为用户数据）：\n")?;
        push(&serde_json::to_string(inputs).map_err(|_| Error::Input)?)?;
        for dependency in &step.depends_on {
            let text = results.get(dependency).ok_or(Error::MissingDependency)?;
            push("\n依赖步骤 ")?;
            push(dependency)?;
            push(" 的已确认结果：\n")?;
            push(text)?;
        }
        if step.expected == Expected::Analysis {
            push(
                "\n仅输出需求分析 JSON：summary、modules（key、label、description、fields）、risks、questions；字段包含 name、label、kind、required。输出必须满足提供的 JSON Schema。",
            )?;
        }
        Ok(result)
    }
    pub fn template(mode: Mode) -> Self {
        let (title, design, deliver) = match mode {
            Mode::Project => (
                "工程设计",
                "根据分析设计全新 Rust/Axum、MongoDB 与 Vue/Tauri 工程，明确模块、接口、权限、数据边界和验证策略。排除客户/客户端管理业务，不加入旧 Go/MySQL 兼容层。",
                "给出可逐步执行的工程实现 Prompt、资源定义、目录与代码、构建和验收命令。模型提案不直接写入文件或管理资源，明确需要用户确认的代码生成步骤。",
            ),
            Mode::Feature => (
                "变更设计",
                "根据需求分析给出功能完善方案，明确受影响模块、不变量、权限和数据变更。使用当前新 Rust/MongoDB/Vue 架构。",
                "给出具体实现 Prompt、变更内容和可复现验证步骤，覆盖失败、取消、重启和权限变化，避免无依据的兼容分支。",
            ),
            Mode::McpGuide => (
                "MCP 协议设计",
                "根据分析设计受控 MCP 接入，明确工具 Schema、当前权限、固定版本引用、资源生命周期和未知结果；不根据模型文字执行工具。",
                "给出 Rust MCP 实现 Prompt、调用示例与验收步骤，保持凭据写入式、工具批准边界、一次效果和恢复策略；所有实际管理操作仍需明确用户操作。",
            ),
        };
        Self{mode,steps:vec![Step{id:"analysis".into(),title:"需求分析".into(),prompt:"分析需求、场景与约束，输出模块及字段建议、风险和待确认问题。不得把建议当作已经完成的实现。".into(),depends_on:vec![],expected:Expected::Analysis},Step{id:"design".into(),title:title.into(),prompt:design.into(),depends_on:vec!["analysis".into()],expected:Expected::Text},Step{id:"deliver".into(),title:"实现与验证".into(),prompt:deliver.into(),depends_on:vec!["analysis".into(),"design".into()],expected:Expected::Text}]}
    }
}
