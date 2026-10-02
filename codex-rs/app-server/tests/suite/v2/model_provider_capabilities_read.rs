use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ModelProviderCapabilitiesReadParams;
use codex_app_server_protocol::ModelProviderCapabilitiesReadResponse;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use test_case::test_case;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[test_case(None, true, false, false; "default_astra")]
#[test_case(Some("openai"), true, true, true; "openai")]
#[test_case(Some("amazon-bedrock"), true, false, true; "amazon_bedrock")]
#[test_case(Some("amazon-bedrock-runtime"), true, false, false; "amazon_bedrock_runtime")]
#[tokio::test]
async fn read_provider_capabilities(
    model_provider: Option<&str>,
    namespace_tools: bool,
    image_generation: bool,
    web_search: bool,
) -> Result<()> {
    let codex_home = TempDir::new()?;
    if let Some(model_provider) = model_provider {
        std::fs::write(
            codex_home.path().join("config.toml"),
            format!("model_provider = \"{model_provider}\"\n"),
        )?;
    }
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_TIMEOUT)
        .await?;

    let request_id = mcp
        .send_model_provider_capabilities_read_request(ModelProviderCapabilitiesReadParams {})
        .await?;
    let received: ModelProviderCapabilitiesReadResponse =
        timeout(DEFAULT_TIMEOUT, mcp.read_response(request_id)).await??;

    assert_eq!(
        received,
        ModelProviderCapabilitiesReadResponse {
            namespace_tools,
            image_generation,
            web_search,
        }
    );
    Ok(())
}
