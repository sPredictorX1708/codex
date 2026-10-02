use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_5_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_6_LUNA_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_6_SOL_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_6_TERRA_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_6_LUNA_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_6_SOL_MODEL_ID;
use codex_models_manager::bundled_models_response;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelServiceTier;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::WebSearchToolType;
use codex_protocol::protocol::MultiAgentVersion;

const GPT_5_BEDROCK_CONTEXT_WINDOW: i64 = 272_000;
const GPT_5_6_SOL_OPENAI_MODEL_ID: &str = "gpt-5.6-sol";
const GPT_5_6_TERRA_OPENAI_MODEL_ID: &str = "gpt-5.6-terra";
const GPT_5_6_LUNA_OPENAI_MODEL_ID: &str = "gpt-5.6-luna";
const GPT_6_SOL_OPENAI_MODEL_ID: &str = "gpt-6-sol";
const GPT_6_LUNA_OPENAI_MODEL_ID: &str = "gpt-6-luna";
const GPT_6_ASTRA_OPENAI_MODEL_ID: &str = "gpt-6-astra";
const GPT_5_5_OPENAI_MODEL_ID: &str = "gpt-5.5";
/// Bedrock's speed tier for GPT-6 Astra, sent as `"service_tier": "ultrafast"`.
const ULTRAFAST_SERVICE_TIER_ID: &str = "ultrafast";
/// Code-mode batching rules in the bundled GPT-6 instruction templates. They name the
/// `functions.exec` tool, which Bedrock models do not get because `bedrock_model` clears
/// `tool_mode`.
const CODE_MODE_BATCHING_RULES: &str = "- Batch independent searches and reads in one functions.exec using await Promise.allSettled([...]); inspect every result. Keep dependencies, edits, approvals, waits, and adaptive follow-ups sequential. Avoid unnecessary output.\n- When calling `functions.exec`, parallelize independent tool calls by awaiting Promises. Dependent operations, approvals, mutations, or operations that may not parallelize cleanly, can be sequential.\n";
/// The same batching rule phrased for the direct `exec_command` tool Bedrock models run with.
const DIRECT_TOOL_BATCHING_RULE: &str = "- Batch independent searches, file reads, and probe scripts into one exec_command call, joining the commands with newlines or `;` rather than `&&` so one failing part does not skip the rest; inspect every result. Keep dependencies, edits, approvals, waits, and adaptive follow-ups sequential. Avoid unnecessary output.\n";

pub(crate) fn static_model_catalog() -> ModelsResponse {
    normalize_bedrock_catalog(ModelsResponse {
        models: vec![
            bedrock_model(
                bundled_openai_model(GPT_6_SOL_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_6_SOL_MODEL_ID,
                "GPT-6 Sol",
                /*priority*/ 0,
            ),
            bedrock_model(
                bundled_openai_model(GPT_6_ASTRA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID,
                "GPT-6-Astra",
                /*priority*/ 1,
            ),
            bedrock_model(
                bundled_openai_model(GPT_6_LUNA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_6_LUNA_MODEL_ID,
                "GPT-6 Luna",
                /*priority*/ 2,
            ),
            bedrock_model(
                bundled_openai_model(GPT_5_6_SOL_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_5_6_SOL_MODEL_ID,
                "GPT-5.6 Sol",
                /*priority*/ 3,
            ),
            bedrock_model(
                bundled_openai_model(GPT_5_6_TERRA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_5_6_TERRA_MODEL_ID,
                "GPT-5.6 Terra",
                /*priority*/ 4,
            ),
            bedrock_model(
                bundled_openai_model(GPT_5_6_LUNA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_5_6_LUNA_MODEL_ID,
                "GPT-5.6 Luna",
                /*priority*/ 5,
            ),
            gpt_5_bedrock_model(
                GPT_5_5_OPENAI_MODEL_ID,
                AMAZON_BEDROCK_GPT_5_5_MODEL_ID,
                "GPT-5.5",
                /*priority*/ 6,
            ),
        ],
    })
}

pub(super) fn static_gov_model_catalog() -> ModelsResponse {
    let mut catalog = static_model_catalog();
    catalog.models.retain(|model| {
        matches!(
            model.slug.as_str(),
            AMAZON_BEDROCK_GPT_5_6_TERRA_MODEL_ID | AMAZON_BEDROCK_GPT_5_6_LUNA_MODEL_ID
        )
    });
    catalog
}

/// Catalog for the local `astra` provider: the Bedrock Mantle model set with
/// GPT-6 Astra promoted to the default (lowest priority value wins).
pub(crate) fn astra_model_catalog() -> ModelsResponse {
    let mut catalog = static_model_catalog();
    // The local proxy is only meant to serve GPT-6 Astra: hide the other
    // Bedrock models from `/model` and make Astra the default at `high`.
    catalog
        .models
        .retain(|model| model.slug == AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID);
    for model in &mut catalog.models {
        model.priority = 0;
        model.context_window = Some(400_000);
        model.default_reasoning_level = Some(ReasoningEffort::High);
    }
    catalog
}

pub(crate) fn normalize_bedrock_catalog(mut catalog: ModelsResponse) -> ModelsResponse {
    for model in &mut catalog.models {
        // Amazon Bedrock offers no Priority or Flex tier for GPT models; GPT-6 Astra alone adds
        // the opt-in Ultrafast tier.
        model.additional_speed_tiers.clear();
        model.service_tiers.clear();
        model.default_service_tier = None;
        if is_gpt_6_astra(&model.slug) {
            model.service_tiers.push(ultrafast_service_tier());
        }
        // Bedrock rejects the `search_content_types` field used by multimodal search.
        model.web_search_tool_type = WebSearchToolType::Text;
        // Bedrock does not support the response items used by multi-agent V2.
        model.multi_agent_version = Some(MultiAgentVersion::V1);
        // Bedrock rejects `reasoning.summary` with `unsupported_parameter`.
        model.supports_reasoning_summary_parameter = false;
        model.default_reasoning_summary = ReasoningSummary::None;
    }
    catalog
}

/// Matches GPT-6 Astra on Mantle (`openai.gpt-6-astra`) and on its `us.` and `global.`
/// cross-Region inference profiles.
fn is_gpt_6_astra(slug: &str) -> bool {
    let slug = slug
        .strip_prefix("us.")
        .or_else(|| slug.strip_prefix("global."))
        .unwrap_or(slug);
    slug == AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID
}

fn ultrafast_service_tier() -> ModelServiceTier {
    ModelServiceTier {
        id: ULTRAFAST_SERVICE_TIER_ID.to_string(),
        name: "Ultrafast".to_string(),
        description: "Up to 6x faster at 6x the price; Mantle us-east-1 or cross-Region only"
            .to_string(),
    }
}

fn gpt_5_bedrock_model(
    openai_slug: &str,
    bedrock_slug: &str,
    display_name: &str,
    priority: i32,
) -> ModelInfo {
    let mut model = bundled_openai_model(openai_slug);
    model.slug = bedrock_slug.to_string();
    model.display_name = display_name.to_string();
    model.priority = priority;
    model.context_window = Some(GPT_5_BEDROCK_CONTEXT_WINDOW);
    model.max_context_window = Some(GPT_5_BEDROCK_CONTEXT_WINDOW);
    model.visibility = ModelVisibility::List;
    model.availability_nux = None;
    model.upgrade = None;
    model
}

fn bedrock_model(
    mut model: ModelInfo,
    bedrock_slug: &str,
    display_name: &str,
    priority: i32,
) -> ModelInfo {
    model.slug = bedrock_slug.to_string();
    model.display_name = display_name.to_string();
    model.priority = priority;
    model.visibility = ModelVisibility::List;
    model.availability_nux = None;
    model.upgrade = None;
    model.use_responses_lite = false;
    model.tool_mode = None;
    use_direct_tool_batching_rule(&mut model);
    model
        .supported_reasoning_levels
        .retain(|level| level.effort != ReasoningEffort::Ultra);
    model
}

fn use_direct_tool_batching_rule(model: &mut ModelInfo) {
    if let Some(template) = model
        .model_messages
        .as_mut()
        .and_then(|messages| messages.instructions_template.as_mut())
    {
        *template = template.replace(CODE_MODE_BATCHING_RULES, DIRECT_TOOL_BATCHING_RULE);
    }
}

fn bundled_openai_model(slug: &str) -> ModelInfo {
    bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"))
        .models
        .into_iter()
        .find(|model| model.slug == slug)
        .unwrap_or_else(|| panic!("bundled models.json should include {slug}"))
}

#[cfg(test)]
mod tests {
    use codex_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn catalog_uses_mantle_model_ids_in_priority_order() {
        let catalog = static_model_catalog();

        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            vec![
                AMAZON_BEDROCK_GPT_6_SOL_MODEL_ID,
                AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID,
                AMAZON_BEDROCK_GPT_6_LUNA_MODEL_ID,
                AMAZON_BEDROCK_GPT_5_6_SOL_MODEL_ID,
                AMAZON_BEDROCK_GPT_5_6_TERRA_MODEL_ID,
                AMAZON_BEDROCK_GPT_5_6_LUNA_MODEL_ID,
                AMAZON_BEDROCK_GPT_5_5_MODEL_ID,
            ]
        );
    }

    #[test]
    fn gpt_5_bedrock_models_use_bedrock_context_window() {
        let catalog = static_model_catalog();

        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| (
                    model.slug.as_str(),
                    model.context_window,
                    model.max_context_window,
                    model.web_search_tool_type,
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    AMAZON_BEDROCK_GPT_6_SOL_MODEL_ID,
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    Some(872_000),
                    WebSearchToolType::Text,
                ),
                (
                    AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID,
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    Some(872_000),
                    WebSearchToolType::Text,
                ),
                (
                    AMAZON_BEDROCK_GPT_6_LUNA_MODEL_ID,
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    Some(872_000),
                    WebSearchToolType::Text,
                ),
                (
                    AMAZON_BEDROCK_GPT_5_6_SOL_MODEL_ID,
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    Some(872_000),
                    WebSearchToolType::Text,
                ),
                (
                    AMAZON_BEDROCK_GPT_5_6_TERRA_MODEL_ID,
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    Some(872_000),
                    WebSearchToolType::Text,
                ),
                (
                    AMAZON_BEDROCK_GPT_5_6_LUNA_MODEL_ID,
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    Some(872_000),
                    WebSearchToolType::Text,
                ),
                (
                    AMAZON_BEDROCK_GPT_5_5_MODEL_ID,
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    Some(GPT_5_BEDROCK_CONTEXT_WINDOW),
                    WebSearchToolType::Text,
                ),
            ]
        );
    }

    #[test]
    fn configured_bedrock_catalogs_normalize_unsupported_model_capabilities() {
        let model = bundled_openai_model(GPT_5_5_OPENAI_MODEL_ID);
        let mut expected = model.clone();
        expected.additional_speed_tiers.clear();
        expected.service_tiers.clear();
        expected.default_service_tier = None;
        expected.web_search_tool_type = WebSearchToolType::Text;
        expected.multi_agent_version = Some(MultiAgentVersion::V1);
        expected.supports_reasoning_summary_parameter = false;

        assert_eq!(
            normalize_bedrock_catalog(ModelsResponse {
                models: vec![model],
            }),
            ModelsResponse {
                models: vec![expected],
            }
        );
    }

    #[test]
    fn gpt_5_bedrock_models_are_visible_without_availability_nux_or_upgrade() {
        for model in static_model_catalog().models {
            assert_eq!(
                (model.visibility, model.availability_nux, model.upgrade),
                (ModelVisibility::List, None, None)
            );
        }
    }

    #[test]
    fn bedrock_models_preserve_source_metadata_with_supported_capabilities() {
        let catalog = static_model_catalog();

        for (mut expected, slug, display_name, priority) in [
            (
                bundled_openai_model(GPT_6_SOL_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_6_SOL_MODEL_ID,
                "GPT-6 Sol",
                0,
            ),
            (
                bundled_openai_model(GPT_6_LUNA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_6_LUNA_MODEL_ID,
                "GPT-6 Luna",
                2,
            ),
            (
                bundled_openai_model(GPT_5_6_SOL_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_5_6_SOL_MODEL_ID,
                "GPT-5.6 Sol",
                3,
            ),
            (
                bundled_openai_model(GPT_5_6_TERRA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_5_6_TERRA_MODEL_ID,
                "GPT-5.6 Terra",
                4,
            ),
            (
                bundled_openai_model(GPT_5_6_LUNA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_5_6_LUNA_MODEL_ID,
                "GPT-5.6 Luna",
                5,
            ),
            (
                bundled_openai_model(GPT_6_ASTRA_OPENAI_MODEL_ID),
                AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID,
                "GPT-6-Astra",
                1,
            ),
        ] {
            expected.slug = slug.to_string();
            expected.display_name = display_name.to_string();
            expected.priority = priority;
            expected.visibility = ModelVisibility::List;
            expected.availability_nux = None;
            expected.upgrade = None;
            expected.use_responses_lite = false;
            expected.tool_mode = None;
            use_direct_tool_batching_rule(&mut expected);
            expected
                .supported_reasoning_levels
                .retain(|level| level.effort != ReasoningEffort::Ultra);
            expected.additional_speed_tiers.clear();
            expected.service_tiers.clear();
            if slug == AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID {
                expected.service_tiers.push(ultrafast_service_tier());
            }
            expected.default_service_tier = None;
            expected.web_search_tool_type = WebSearchToolType::Text;
            expected.multi_agent_version = Some(MultiAgentVersion::V1);
            expected.supports_reasoning_summary_parameter = false;

            assert_eq!(
                catalog.models.iter().find(|model| model.slug == slug),
                Some(&expected)
            );
        }
    }

    #[test]
    fn bedrock_gpt_6_instructions_use_direct_tool_batching_rule() {
        let catalog = static_model_catalog();

        for (openai_slug, bedrock_slug) in [
            (GPT_6_SOL_OPENAI_MODEL_ID, AMAZON_BEDROCK_GPT_6_SOL_MODEL_ID),
            (
                GPT_6_ASTRA_OPENAI_MODEL_ID,
                AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID,
            ),
            (
                GPT_6_LUNA_OPENAI_MODEL_ID,
                AMAZON_BEDROCK_GPT_6_LUNA_MODEL_ID,
            ),
        ] {
            let source = bundled_openai_model(openai_slug)
                .model_messages
                .and_then(|messages| messages.instructions_template)
                .unwrap_or_default();
            let bedrock = catalog
                .models
                .iter()
                .find(|model| model.slug == bedrock_slug)
                .and_then(|model| model.model_messages.as_ref())
                .and_then(|messages| messages.instructions_template.as_deref())
                .unwrap_or_default();

            assert_eq!(
                (
                    source.contains(CODE_MODE_BATCHING_RULES),
                    bedrock.contains("functions.exec"),
                    bedrock.contains("Promise"),
                    bedrock.matches(DIRECT_TOOL_BATCHING_RULE).count(),
                ),
                (true, false, false, 1),
                "{bedrock_slug}"
            );
            assert_eq!(
                bedrock,
                source.replace(CODE_MODE_BATCHING_RULES, DIRECT_TOOL_BATCHING_RULE),
                "{bedrock_slug}"
            );
        }
    }

    #[test]
    fn gpt_5_bedrock_models_only_allow_default_service_tier() {
        let catalog = static_model_catalog();

        for model in catalog
            .models
            .into_iter()
            .filter(|model| model.slug != AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID)
        {
            assert_eq!(model.additional_speed_tiers, Vec::<String>::new());
            assert_eq!(model.service_tiers, Vec::new());
            assert_eq!(model.default_service_tier, None);
            assert_eq!(
                model.service_tier_for_request(Some("priority".to_string())),
                None
            );
            assert_eq!(
                model
                    .service_tier_for_request(Some(SERVICE_TIER_DEFAULT_REQUEST_VALUE.to_string())),
                None
            );
        }
    }

    #[test]
    fn gpt_6_astra_bedrock_models_offer_only_the_ultrafast_service_tier() {
        let configured_astra = ["", "us.", "global."].map(|prefix| {
            let mut model = bundled_openai_model(GPT_6_ASTRA_OPENAI_MODEL_ID);
            model.slug = format!("{prefix}{AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID}");
            model
        });
        let models = static_model_catalog()
            .models
            .into_iter()
            .chain(astra_model_catalog().models)
            .chain(
                normalize_bedrock_catalog(ModelsResponse {
                    models: configured_astra.to_vec(),
                })
                .models,
            )
            .filter(|model| model.slug.ends_with(AMAZON_BEDROCK_GPT_6_ASTRA_MODEL_ID))
            .collect::<Vec<_>>();

        assert_eq!(models.len(), 5);
        for model in models {
            assert_eq!(
                (
                    model
                        .service_tiers
                        .iter()
                        .map(|tier| tier.id.as_str())
                        .collect::<Vec<_>>(),
                    model.default_service_tier.as_deref(),
                    model.additional_speed_tiers.clone(),
                    model.service_tier_for_request(Some("ultrafast".to_string())),
                    model.service_tier_for_request(Some("priority".to_string())),
                    model.service_tier_for_request(Some(
                        SERVICE_TIER_DEFAULT_REQUEST_VALUE.to_string()
                    )),
                ),
                (
                    vec!["ultrafast"],
                    None,
                    Vec::<String>::new(),
                    Some("ultrafast".to_string()),
                    None,
                    None,
                )
            );
        }
    }
}
