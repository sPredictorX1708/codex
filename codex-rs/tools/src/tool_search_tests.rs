use super::*;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[test]
fn shared_search_specs_preserve_results_and_release_the_source() {
    let function = ResponsesApiTool {
        name: "lookup".to_string(),
        description: "Look up an item".to_string(),
        strict: true,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::new(),
            /*required*/ None,
            /*additional_properties*/ None,
        ),
        output_schema: Some(serde_json::json!({"type": "object"}).into()),
    };
    let custom = crate::FreeformTool {
        name: "patch".to_string(),
        description: "Apply a patch".to_string(),
        defer_loading: None,
        format: crate::FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: \"patch\"".to_string(),
        },
    };
    for spec in [
        ToolSpec::Function(function.clone()),
        ToolSpec::Freeform(custom.clone()),
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "example".to_string(),
            description: String::new(),
            tools: vec![
                ResponsesApiNamespaceTool::Function(function),
                ResponsesApiNamespaceTool::Custom(custom),
            ],
        }),
    ] {
        let expected =
            ToolSearchInfo::from_spec("query".to_string(), spec.clone(), /*source_info*/ None)
                .unwrap()
                .entry
                .to_loadable_spec();
        let spec = Arc::new(spec);
        let weak = Arc::downgrade(&spec);
        let info = ToolSearchInfo::from_shared_spec(
            "query".to_string(),
            Arc::clone(&spec),
            /*source_info*/ None,
        )
        .unwrap();
        drop(spec);
        assert!(
            weak.upgrade().is_some(),
            "search retains the original spec rather than a deep copy"
        );
        assert_eq!(info.entry.to_loadable_spec(), expected);
        drop(info);
        assert!(
            weak.upgrade().is_none(),
            "search does not keep discarded catalogs alive"
        );
    }
}

#[test]
fn identifier_queries_match_callables_without_matching_description_words() {
    let spec = ToolSpec::Namespace(ResponsesApiNamespace {
        name: "mcp__archive".to_string(),
        description: "ReadArchivePages and policy records".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "ReadArchivePages".to_string(),
            description: "Read archived pages with GetPolicyRecord.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        })],
    });
    let info = ToolSearchInfo::from_tool_spec(
        spec,
        Some(ToolSearchSourceInfo {
            name: "archive-service".to_string(),
            description: None,
        }),
    )
    .unwrap();
    for query in [
        "ReadArchivePages",
        "readarchivepages",
        " mcp__archive.ReadArchivePages ",
        "mcp__archive__ReadArchivePages",
        "+archive ReadArchivePages",
        "+ARCHIVE-SERVICE readarchivepages",
        "+mcp__archive\tReadArchivePages",
    ] {
        assert!(info.matches_identifier_query(query), "{query}");
    }
    for query in [
        "",
        "archive",
        "GetPolicyRecord",
        "read archived pages",
        "+other ReadArchivePages",
        "+archive ReadArchivePages policy",
        "ReadArchivePages policy",
    ] {
        assert!(!info.matches_identifier_query(query), "{query}");
    }
}

#[test]
fn top_level_function_search_results_use_the_default_namespace() {
    let function_tool = ResponsesApiTool {
        name: "lookup_order".to_string(),
        description: "Look up an order".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::new(),
            /*required*/ None,
            /*additional_properties*/ None,
        ),
        output_schema: Some(serde_json::json!({ "type": "object" }).into()),
    };
    let search_info = ToolSearchInfo::from_tool_spec(
        ToolSpec::Function(function_tool.clone()),
        /*source_info*/ None,
    )
    .expect("top-level function should be searchable");

    assert!(search_info.matches_identifier_query("lookup_order"));
    assert!(search_info.matches_identifier_query("functions.lookup_order"));
    assert_eq!(
        (
            search_info.entry.search_text.clone(),
            search_info.entry.to_loadable_spec()
        ),
        (
            "lookup_order lookup order Look up an order".to_string(),
            LoadableToolSpec::Namespace(ResponsesApiNamespace {
                name: "functions".to_string(),
                description: String::new(),
                tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                    defer_loading: Some(true),
                    output_schema: None,
                    ..function_tool
                })],
            }),
        )
    );
}

#[test]
fn top_level_custom_tools_are_searchable() {
    let custom_tool = crate::FreeformTool {
        name: "apply_patch".to_string(),
        description: "Apply a patch".to_string(),
        defer_loading: None,
        format: crate::FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: \"patch\"".to_string(),
        },
    };
    let search_info = ToolSearchInfo::from_tool_spec(
        ToolSpec::Freeform(custom_tool.clone()),
        /*source_info*/ None,
    )
    .expect("top-level custom tool should be searchable");

    assert!(search_info.matches_identifier_query("apply_patch"));
    assert!(search_info.matches_identifier_query("+functions apply_patch"));
    assert_eq!(
        (
            search_info.entry.search_text.clone(),
            search_info.entry.to_loadable_spec()
        ),
        (
            "apply_patch Apply a patch lark".to_string(),
            LoadableToolSpec::Namespace(ResponsesApiNamespace {
                name: "functions".to_string(),
                description: String::new(),
                tools: vec![ResponsesApiNamespaceTool::Custom(crate::FreeformTool {
                    defer_loading: Some(true),
                    ..custom_tool
                })],
            }),
        )
    );
}

#[test]
fn default_search_text_uses_model_visible_namespace_metadata_once() {
    let mut schedule_schema = JsonSchema::object(
        BTreeMap::from([(
            "timezone".to_string(),
            JsonSchema::string(Some("IANA timezone.".to_string())),
        )]),
        /*required*/ None,
        /*additional_properties*/ None,
    );
    schedule_schema.description = Some("Schedule settings.".to_string());
    let mut parameters = JsonSchema::object(
        BTreeMap::from([
            (
                "mode".to_string(),
                JsonSchema::string(Some("Update mode.".to_string())),
            ),
            ("schedule".to_string(), schedule_schema),
        ]),
        /*required*/ None,
        /*additional_properties*/ None,
    );
    parameters.description = Some("Automation options.".to_string());
    let spec = ToolSpec::Namespace(crate::ResponsesApiNamespace {
        name: "codex_app".to_string(),
        description: "Manage Codex automations.".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "automation_update".to_string(),
            description: "Create or update automations.".to_string(),
            strict: false,
            defer_loading: None,
            parameters,
            output_schema: None,
        })],
    });

    let search_info = ToolSearchInfo::from_tool_spec(spec, /*source_info*/ None)
        .expect("namespace should be searchable");

    assert_eq!(
        search_info.entry.search_text,
        "codex_app Manage Codex automations. automation_update automation update Create or update automations. Automation options. mode Update mode. schedule Schedule settings. timezone IANA timezone."
    );
}

#[test]
fn mixed_namespaced_function_and_custom_tools_are_searchable() {
    let function_tool = ResponsesApiTool {
        name: "lookup_order".to_string(),
        description: "Look up an order".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::new(),
            /*required*/ None,
            /*additional_properties*/ None,
        ),
        output_schema: Some(serde_json::json!({"type": "object"}).into()),
    };
    let custom_tool = crate::FreeformTool {
        name: "apply_patch".to_string(),
        description: "Apply a patch".to_string(),
        defer_loading: None,
        format: crate::FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: \"patch\"".to_string(),
        },
    };
    let spec = ToolSpec::Namespace(crate::ResponsesApiNamespace {
        name: "editor".to_string(),
        description: "Editing tools".to_string(),
        tools: vec![
            ResponsesApiNamespaceTool::Function(function_tool.clone()),
            ResponsesApiNamespaceTool::Custom(custom_tool.clone()),
        ],
    });

    let search_info = ToolSearchInfo::from_tool_spec(spec, /*source_info*/ None)
        .expect("mixed namespace should be searchable");

    assert_eq!(
        search_info.entry.search_text,
        "editor Editing tools lookup_order lookup order Look up an order apply_patch Apply a patch lark"
    );
    assert_eq!(
        search_info.entry.to_loadable_spec(),
        LoadableToolSpec::Namespace(crate::ResponsesApiNamespace {
            name: "editor".to_string(),
            description: "Editing tools".to_string(),
            tools: vec![
                ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                    defer_loading: Some(true),
                    output_schema: None,
                    ..function_tool
                }),
                ResponsesApiNamespaceTool::Custom(crate::FreeformTool {
                    defer_loading: Some(true),
                    ..custom_tool
                }),
            ],
        })
    );
}
