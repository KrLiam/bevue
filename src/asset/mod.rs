use bevy::{
    app::{App, Plugin},
    asset::{io::Reader, AssetApp, AssetLoader, AssetPath, AssetServer, Assets, LoadContext},
    ecs::resource::Resource,
    platform::collections::HashMap,
    reflect::TypePath,
};
use bevy_flair::style::StyleSheet;

use std::path::{Path, PathBuf};

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

/// The stylesheets Bevue generated for rendered templates, by their path in Bevy's `embedded://`
/// asset source (e.g. `bevue/inventory.html.css`).
#[derive(Resource, Default, Debug)]
pub struct BevueStylesheets {
    sheets: HashMap<PathBuf, String>,
}

impl BevueStylesheets {
    /// The asset source prefix of generated stylesheets.
    pub const ROOT: &'static str = "bevue";

    /// The CSS generated for `path`.
    pub fn get(&self, path: impl AsRef<Path>) -> Option<&str> {
        self.sheets.get(path.as_ref()).map(String::as_str)
    }

    /// Stores `css` as the stylesheet `path`. Returns false if it already had this content.
    pub(crate) fn set(&mut self, path: &Path, css: &str) -> bool {
        if self.get(path) == Some(css) {
            return false;
        }
        self.sheets.insert(path.to_owned(), css.to_owned());
        true
    }
}

/// Plugin that initializes the `BevueTemplate` asset and registers its loader. Add it after Bevy's
/// `AssetPlugin`.
pub struct BevueAssetPlugin;

impl Plugin for BevueAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BevueStylesheets>();
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
        asset::{AssetPlugin, AssetServer, Assets},
        MinimalPlugins,
    };

    #[test]
    fn test_asset_loader_registered() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueAssetPlugin));
        app.finish();

        let world = app.world();
        assert!(world.contains_resource::<Assets<BevueTemplate>>());
        assert!(world.contains_resource::<AssetServer>());
        assert!(world.contains_resource::<BevueStylesheets>());
    }
}
