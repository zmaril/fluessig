//! The op-layer IR — serde mirror of `api.json` (format 0, frozen by
//! `emitter/api.schema.json`): interfaces, ops with shapes, params, returns,
//! and the DTO models the ops reference. The input to [`crate::bindgen`].

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiDoc {
    pub fluessig: crate::ir::Versions,
    #[serde(default)]
    pub source: Option<String>,
    pub models: Vec<ApiModel>,
    pub interfaces: Vec<ApiInterface>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiModel {
    pub name: String,
    #[serde(default)]
    pub doc: Option<String>,
    pub fields: Vec<ApiField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ApiType,
    pub nullable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiInterface {
    pub name: String,
    #[serde(default)]
    pub doc: Option<String>,
    pub ops: Vec<ApiOp>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiOp {
    pub name: String,
    #[serde(default)]
    pub doc: Option<String>,
    pub shape: Shape,
    pub params: Vec<ApiParam>,
    pub returns: ApiType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Ctor,
    Unary,
    Stream,
    Manual,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiParam {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ApiType,
    #[serde(default)]
    pub optional: Option<bool>,
}

/// A type in the op surface: a scalar name (or `"void"`), a model/enum
/// reference, or a list thereof.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ApiType {
    Scalar(String),
    Model {
        model: String,
    },
    Enum {
        r#enum: String,
    },
    List {
        list: Box<ApiType>,
    },
    /// `T | null` — nullable returns/params.
    Nullable {
        nullable: Box<ApiType>,
    },
}

/// Parse `api.json` (with the same format-version gate as the catalog).
pub fn load_api(json: &str) -> Result<ApiDoc, String> {
    let api: ApiDoc =
        serde_json::from_str(json).map_err(|e| format!("api.json parse error: {e}"))?;
    if api.fluessig.format != crate::FORMAT_VERSION {
        return Err(format!(
            "api format {} is not supported (this fluessig reads format {})",
            api.fluessig.format,
            crate::FORMAT_VERSION
        ));
    }
    Ok(api)
}

/// [`load_api`] from a file path.
pub fn load_api_file(path: impl AsRef<std::path::Path>) -> Result<ApiDoc, String> {
    let json = std::fs::read_to_string(path.as_ref())
        .map_err(|e| format!("read {}: {e}", path.as_ref().display()))?;
    load_api(&json)
}
