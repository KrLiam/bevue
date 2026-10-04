use bevy::{
    app::{App, Plugin},
    asset::{io::Reader, AssetApp, AssetLoader, LoadContext},
    reflect::TypePath,
};

use crate::{core::template::BevueTemplate, parser::parse_template};

#[derive(Debug)]
pub enum BevueTemplateLoaderError {
    Io(std::io::Error),
    Utf8(std::str::Utf8Error),
    Parse(crate::parser::ParseError),
}

impl std::fmt::Display for BevueTemplateLoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BevueTemplateLoaderError::Io(e) => write!(f, "Could not read template file: {e}"),
            BevueTemplateLoaderError::Utf8(e) => write!(f, "Template source is not valid UTF-8: {e}"),
            BevueTemplateLoaderError::Parse(e) => write!(f, "Could not parse template: {e}"),
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
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let source = std::str::from_utf8(&bytes)?;
        let template = parse_template(source)?;
        Ok(template)
    }

    fn extensions(&self) -> &[&str] {
        &["html", "bevue"]
    }
}

/// Plugin that initializes the `BevueTemplate` asset and registers its loader.
pub struct BevueAssetPlugin;

impl Plugin for BevueAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<BevueTemplate>()
            .register_asset_loader(BevueTemplateLoader);
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
    }
}
