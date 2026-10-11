use bevy::{
    app::{App, Plugin},
    asset::{
        io::{
            memory::{Dir, MemoryAssetReader},
            AssetSourceBuilder, Reader,
        },
        AssetApp, AssetLoader, AssetPath, AssetServer, Assets, LoadContext,
    },
    ecs::resource::Resource,
    prelude::{Deref, DerefMut},
    reflect::TypePath,
};
use bevy_flair::style::StyleSheet;

use crate::{core::template::BevueTemplate, parser::parse_template};

#[derive(Debug)]
pub enum BevueTemplateLoaderError {
    Io(std::io::Error),
    Utf8(std::str::Utf8Error),
    Parse(crate::parser::ParseError),
    Compile(crate::core::compile::CompileError),
    Import(String),
}

impl std::fmt::Display for BevueTemplateLoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BevueTemplateLoaderError::Io(e) => write!(f, "Could not read template file: {e}"),
            BevueTemplateLoaderError::Utf8(e) => write!(f, "Template source is not valid UTF-8: {e}"),
            BevueTemplateLoaderError::Parse(e) => write!(f, "Could not parse template: {e}"),
            BevueTemplateLoaderError::Compile(e) => write!(f, "Invalid template: {e}"),
            BevueTemplateLoaderError::Import(e) => write!(f, "Invalid template: {e}"),
        }
    }
}

impl std::error::Error for BevueTemplateLoaderError {}

impl From<std::io::Error> for BevueTemplateLoaderError {
    fn from(e: std::io::Error) -> Self {
        BevueTemplateLoaderError::Io(e)
    }
}

impl From<std::str::Utf8Error> for BevueTemplateLoaderError {
    fn from(e: std::str::Utf8Error) -> Self {
        BevueTemplateLoaderError::Utf8(e)
    }
}

impl From<crate::parser::ParseError> for BevueTemplateLoaderError {
    fn from(e: crate::parser::ParseError) -> Self {
        BevueTemplateLoaderError::Parse(e)
    }
}

/// Asset loader for Bevue template files (`.html` and `.bevue`).
#[derive(Default, TypePath)]
pub struct BevueTemplateLoader;

impl AssetLoader for BevueTemplateLoader {
    type Asset = BevueTemplate;
    type Settings = ();
    type Error = BevueTemplateLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let source = std::str::from_utf8(&bytes)?;
        let mut template = parse_template(source)?;
        // `./` and `../` are relative to this file; other paths start at the asset root.
        for import in &mut template.imports {
            let path = if import.src.starts_with("./") || import.src.starts_with("../") {
                load_context.path().resolve_embed_str(&import.src)
            } else {
                AssetPath::try_parse(&import.src).map(AssetPath::into_owned)
            }
            .map_err(|e| BevueTemplateLoaderError::Import(format!("invalid path in <import src=\"{}\">: {e}", import.src)))?;
            import.handle = Some(load_context.load(path));
        }
        // Report directive and scoping errors at load time rather than at render time.
        crate::core::compile::CompiledTemplate::compile(&template).map_err(BevueTemplateLoaderError::Compile)?;
        Ok(template)
    }

    fn extensions(&self) -> &[&str] {
        &["html", "bevue"]
    }
}

/// Resource storing an in-memory virtual filesystem [`Dir`] for Bevue assets (such as fabricated CSS stylesheets).
#[derive(Resource, Clone, Default, Debug, Deref, DerefMut)]
pub struct BevueAssetRegistry {
    pub dir: Dir,
}

impl BevueAssetRegistry {
    pub fn new() -> Self {
        Self {
            dir: Dir::default(),
        }
    }
}

pub type BevueAssets = BevueAssetRegistry;

/// Plugin that initializes the `BevueTemplate` asset, registers the `"bevue"` virtual asset source, and registers its loader.
pub struct BevueAssetPlugin;

impl Plugin for BevueAssetPlugin {
    fn build(&self, app: &mut App) {
        let registry = app
            .world()
            .get_resource::<BevueAssetRegistry>()
            .cloned()
            .unwrap_or_default();

        let dir = registry.dir.clone();
        app.register_asset_source(
            "bevue",
            AssetSourceBuilder::new(move || Box::new(MemoryAssetReader { root: dir.clone() })),
        );
        app.insert_resource(registry);

        if app.world().get_resource::<AssetServer>().is_some() {
            Self::init_assets(app);
        }
    }

    fn finish(&self, app: &mut App) {
        Self::init_assets(app);
    }
}

impl BevueAssetPlugin {
    fn init_assets(app: &mut App) {
        if !app.world().contains_resource::<Assets<BevueTemplate>>() {
            app.init_asset::<BevueTemplate>()
                .register_asset_loader(BevueTemplateLoader);
        }
        if !app.world().contains_resource::<Assets<StyleSheet>>() {
            app.init_asset::<StyleSheet>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::{AssetPlugin, AssetServer, Assets, Handle},
        MinimalPlugins,
    };

    #[test]
    fn test_asset_loader_registered() {
        let mut app = App::new();
        app.add_plugins((BevueAssetPlugin, MinimalPlugins, AssetPlugin::default()));
        app.finish();

        let world = app.world();
        assert!(world.contains_resource::<Assets<BevueTemplate>>());
        assert!(world.contains_resource::<AssetServer>());
        assert!(world.contains_resource::<BevueAssetRegistry>());
    }

    #[test]
    fn test_bevue_asset_source_registered() {
        let mut app = App::new();
        app.add_plugins((BevueAssetPlugin, MinimalPlugins, AssetPlugin::default()));
        app.finish();

        let registry = app.world().resource::<BevueAssetRegistry>();
        registry.dir.insert_asset_text(
            std::path::Path::new("test.html.css"),
            ".root { color: red; }",
        );

        let asset_server = app.world().resource::<AssetServer>();
        let handle: Handle<bevy_flair::style::StyleSheet> =
            asset_server.load("bevue://test.html.css");
        assert_eq!(handle.path().unwrap().source().as_str(), Some("bevue"));
        assert_eq!(
            handle.path().unwrap().path(),
            std::path::Path::new("test.html.css")
        );
    }
}
