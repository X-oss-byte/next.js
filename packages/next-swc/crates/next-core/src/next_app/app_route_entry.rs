use anyhow::{bail, Result};
use indexmap::indexmap;
use turbo_tasks::{Value, ValueToString, Vc};
use turbopack_binding::{
    turbo::tasks_fs::{rope::RopeBuilder, File, FileSystemPath},
    turbopack::{
        core::{
            asset::AssetContent,
            context::AssetContext,
            module::Module,
            reference_type::{
                EcmaScriptModulesReferenceSubType, EntryReferenceSubType, ReferenceType,
            },
            source::Source,
            virtual_source::VirtualSource,
        },
        ecmascript::{chunk::EcmascriptChunkPlaceable, utils::StringifyJs},
        turbopack::ModuleAssetContext,
    },
};

use crate::{
    next_app::AppEntry,
    parse_segment_config_from_source,
    util::{load_next_js, resolve_next_module, NextRuntime},
};

/// Computes the entry for a Next.js app route.
#[turbo_tasks::function]
pub async fn get_app_route_entry(
    nodejs_context: Vc<ModuleAssetContext>,
    edge_context: Vc<ModuleAssetContext>,
    source: Vc<Box<dyn Source>>,
    pathname: String,
    project_root: Vc<FileSystemPath>,
) -> Result<Vc<AppEntry>> {
    let config = parse_segment_config_from_source(
        nodejs_context.process(
            source,
            Value::new(ReferenceType::Entry(EntryReferenceSubType::AppRoute)),
        ),
        source,
    );
    let context = if matches!(config.await?.runtime, Some(NextRuntime::Edge)) {
        edge_context
    } else {
        nodejs_context
    };

    let mut result = RopeBuilder::default();

    let original_name = get_original_route_name(&pathname);
    let path = source.ident().path();

    let template_file = "/dist/esm/build/webpack/loaders/next-route-loader/templates/app-route.js";

    // Load the file from the next.js codebase.
    let file = load_next_js(project_root, template_file).await?.await?;

    let mut file = file
        .to_str()?
        .replace(
            "\"VAR_DEFINITION_PAGE\"",
            &StringifyJs(&original_name).to_string(),
        )
        .replace(
            "\"VAR_DEFINITION_PATHNAME\"",
            &StringifyJs(&pathname).to_string(),
        )
        .replace(
            "\"VAR_DEFINITION_FILENAME\"",
            &StringifyJs(&path.file_stem().await?.as_ref().unwrap().clone()).to_string(),
        )
        // TODO(alexkirsz) Is this necessary?
        .replace(
            "\"VAR_DEFINITION_BUNDLE_PATH\"",
            &StringifyJs("").to_string(),
        )
        .replace(
            "\"VAR_ORIGINAL_PATHNAME\"",
            &StringifyJs(&original_name).to_string(),
        )
        .replace(
            "\"VAR_RESOLVED_PAGE_PATH\"",
            &StringifyJs(&path.to_string().await?).to_string(),
        )
        .replace(
            "// INJECT:nextConfigOutput",
            "const nextConfigOutput = \"\"",
        );

    // Ensure that the last line is a newline.
    if !file.ends_with('\n') {
        file.push('\n');
    }

    result.concat(&file.into());

    let file = File::from(result.build());

    let resolve_result = resolve_next_module(project_root, template_file).await?;

    let Some(template_path) = *resolve_result.first_module().await? else {
        bail!("Expected to find module");
    };

    let template_path = template_path.ident().path();

    let virtual_source = VirtualSource::new(template_path, AssetContent::file(file.into()));

    let entry = context.process(
        source,
        Value::new(ReferenceType::EcmaScriptModules(
            EcmaScriptModulesReferenceSubType::Undefined,
        )),
    );

    let inner_assets = indexmap! {
        "VAR_USERLAND".to_string() => entry
    };

    let rsc_entry = context.process(
        Vc::upcast(virtual_source),
        Value::new(ReferenceType::Internal(Vc::cell(inner_assets))),
    );

    let Some(rsc_entry) =
        Vc::try_resolve_downcast::<Box<dyn EcmascriptChunkPlaceable>>(rsc_entry).await?
    else {
        bail!("expected an ECMAScript chunk placeable module");
    };

    Ok(AppEntry {
        pathname: pathname.to_string(),
        original_name,
        rsc_entry,
        config,
    }
    .cell())
}

fn get_original_route_name(pathname: &str) -> String {
    match pathname {
        "/" => "/route".to_string(),
        _ => format!("{}/route", pathname),
    }
}
