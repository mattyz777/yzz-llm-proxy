use serde::Deserialize;


#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    pub models: Vec<KiroModel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroModel {
    pub model_id: String,
}