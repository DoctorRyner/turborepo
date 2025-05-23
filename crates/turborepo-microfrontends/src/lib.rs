#![feature(assert_matches)]
#![deny(clippy::all)]
mod configv1;
mod error;

use std::io;

use biome_deserialize_macros::Deserializable;
use biome_json_parser::JsonParserOptions;
use configv1::ConfigV1;
pub use error::Error;
use turbopath::{
    AbsoluteSystemPath, AbsoluteSystemPathBuf, AnchoredSystemPath, AnchoredSystemPathBuf,
};

/// Currently the default path for a package that provides a configuration.
///
/// This is subject to change at any time.
pub const DEFAULT_MICROFRONTENDS_CONFIG_V1: &str = "microfrontends.json";
pub const DEFAULT_MICROFRONTENDS_CONFIG_V1_ALT: &str = "microfrontends.jsonc";
pub const MICROFRONTENDS_PACKAGE: &str = "@vercel/microfrontends";
pub const SUPPORTED_VERSIONS: &[&str] = ["1"].as_slice();

/// The minimal amount of information Turborepo needs to correctly start a local
/// proxy server for microfrontends
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Config {
    inner: ConfigInner,
    filename: String,
    path: Option<AnchoredSystemPathBuf>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct DevelopmentTask<'a> {
    // The key in the applications object in microfrontends.json
    // This will match package unless packageName is provided
    pub application_name: &'a str,
    pub package: &'a str,
    pub task: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum ConfigInner {
    V1(ConfigV1),
}

impl Config {
    /// Reads config from given path.
    /// Returns `Ok(None)` if the file does not exist
    pub fn load(config_path: &AbsoluteSystemPath) -> Result<Option<Self>, Error> {
        let Some(contents) = config_path.read_existing_to_string()? else {
            return Ok(None);
        };
        let config = Self::from_str(&contents, config_path.as_str())?;
        Ok(Some(config))
    }

    /// Attempts to load a configuration file from the given directory
    /// Returns `Ok(None)` if no configuration is found in the directory
    pub fn load_from_dir(
        repo_root: &AbsoluteSystemPath,
        package_dir: &AnchoredSystemPath,
    ) -> Result<Option<Self>, Error> {
        let absolute_dir = repo_root.resolve(package_dir);
        // we want to try different paths and then do `from_str`
        let Some((contents, path)) = Self::load_v1_dir(&absolute_dir) else {
            return Ok(None);
        };
        let contents = contents?;
        let mut config = Config::from_str(&contents, path.as_str())?;
        config.filename = path
            .file_name()
            .expect("microfrontends config should not be root")
            .to_owned();
        config.set_path(package_dir);
        Ok(Some(config))
    }

    pub fn from_str(input: &str, source: &str) -> Result<Self, Error> {
        #[derive(Deserializable, Default)]
        struct VersionOnly {
            version: Option<String>,
        }
        let (version_only, _errs) = biome_deserialize::json::deserialize_from_json_str(
            input,
            JsonParserOptions::default().with_allow_comments(),
            source,
        )
        .consume();

        let version = match version_only {
            Some(VersionOnly {
                version: Some(version),
            }) => version,
            // Default to version 1 if no version found
            Some(VersionOnly { version: None }) | None => "1".to_string(),
        };

        let inner = match version.as_str() {
            "1" => ConfigV1::from_str(input, source).and_then(|result| match result {
                configv1::ParseResult::Actual(config_v1) => Ok(ConfigInner::V1(config_v1)),
                configv1::ParseResult::Reference(default_app) => Err(Error::ChildConfig {
                    reference: default_app,
                }),
            }),
            version => Err(Error::UnsupportedVersion(version.to_string())),
        }?;
        Ok(Self {
            inner,
            filename: source.to_owned(),
            path: None,
        })
    }

    pub fn development_tasks<'a>(&'a self) -> Box<dyn Iterator<Item = DevelopmentTask<'a>> + 'a> {
        match &self.inner {
            ConfigInner::V1(config_v1) => Box::new(config_v1.development_tasks()),
        }
    }

    pub fn port(&self, name: &str) -> Option<u16> {
        match &self.inner {
            ConfigInner::V1(config_v1) => config_v1.port(name),
        }
    }

    /// Filename of the loaded configuration
    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn path(&self) -> Option<&AnchoredSystemPath> {
        let path = self.path.as_deref()?;
        Some(path)
    }

    pub fn version(&self) -> &'static str {
        match &self.inner {
            ConfigInner::V1(_) => "1",
        }
    }

    fn load_v1_dir(
        dir: &AbsoluteSystemPath,
    ) -> Option<(Result<String, io::Error>, AbsoluteSystemPathBuf)> {
        let load_config =
            |filename: &str| -> Option<(Result<String, io::Error>, AbsoluteSystemPathBuf)> {
                let path = dir.join_component(filename);
                let contents = path.read_existing_to_string().transpose()?;
                Some((contents, path))
            };
        load_config(DEFAULT_MICROFRONTENDS_CONFIG_V1)
            .or_else(|| load_config(DEFAULT_MICROFRONTENDS_CONFIG_V1_ALT))
    }

    /// Sets the path the configuration was loaded from
    pub fn set_path(&mut self, dir: &AnchoredSystemPath) {
        self.path = Some(dir.join_component(&self.filename));
    }
}

#[cfg(test)]
mod test {
    use std::assert_matches::assert_matches;

    use insta::assert_snapshot;
    use tempfile::TempDir;
    use test_case::test_case;

    use super::*;

    #[test]
    fn test_example_parses() {
        let input = include_str!("../fixtures/sample.jsonc");
        let example_config = Config::from_str(input, "something.json");
        assert!(example_config.is_ok());
    }

    #[test]
    fn test_unsupported_version() {
        let input = r#"{"version": "yolo"}"#;
        let err = Config::from_str(input, "something.json").unwrap_err();
        assert_snapshot!(err, @r###"Unsupported microfrontends configuration version: yolo. Supported versions: ["1"]"###);
    }

    fn add_v1_config(dir: &AbsoluteSystemPath) -> Result<(), std::io::Error> {
        let path = dir.join_component(DEFAULT_MICROFRONTENDS_CONFIG_V1);
        path.ensure_dir()?;
        path.create_with_contents(r#"{"version": "1", "applications": {"web": {"development": {"task": "serve"}}, "docs": {}}}"#)
    }

    fn add_no_version_config(dir: &AbsoluteSystemPath) -> Result<(), std::io::Error> {
        let path = dir.join_component(DEFAULT_MICROFRONTENDS_CONFIG_V1);
        path.ensure_dir()?;
        path.create_with_contents(
            r#"{"applications": {"web": {"development": {"task": "serve"}}, "docs": {}}}"#,
        )
    }

    fn add_v2_config(dir: &AbsoluteSystemPath) -> Result<(), std::io::Error> {
        let path = dir.join_component(DEFAULT_MICROFRONTENDS_CONFIG_V1);
        path.ensure_dir()?;
        path.create_with_contents(r#"{"version": "2", "applications": {"web": {"development": {"task": "serve"}}, "docs": {}}}"#)
    }

    fn add_v1_alt_config(dir: &AbsoluteSystemPath) -> Result<(), std::io::Error> {
        let path = dir.join_component(DEFAULT_MICROFRONTENDS_CONFIG_V1_ALT);
        path.ensure_dir()?;
        path.create_with_contents(r#"{"version": "1", "applications": {"web": {"development": {"task": "serve"}}, "docs": {}}}"#)
    }

    struct LoadDirTest {
        has_v1: bool,
        has_alt_v1: bool,
        has_versionless: bool,
        pkg_dir: &'static str,
        expected_version: Option<FoundConfig>,
        expected_filename: Option<&'static str>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FoundConfig {
        V1,
    }

    impl LoadDirTest {
        pub const fn new(pkg_dir: &'static str) -> Self {
            Self {
                pkg_dir,
                has_v1: false,
                has_alt_v1: false,
                has_versionless: false,
                expected_version: None,
                expected_filename: None,
            }
        }

        pub const fn has_v1(mut self) -> Self {
            self.has_v1 = true;
            self
        }

        pub const fn has_alt_v1(mut self) -> Self {
            self.has_alt_v1 = true;
            self
        }

        pub const fn has_versionless(mut self) -> Self {
            self.has_versionless = true;
            self
        }

        pub const fn expects_v1(mut self) -> Self {
            self.expected_version = Some(FoundConfig::V1);
            self
        }

        pub const fn with_filename(mut self, filename: &'static str) -> Self {
            self.expected_filename = Some(filename);
            self
        }

        pub fn expected_path(&self) -> Option<AnchoredSystemPathBuf> {
            let filename = self.expected_filename?;
            Some(
                AnchoredSystemPath::new(self.pkg_dir)
                    .unwrap()
                    .join_component(filename),
            )
        }
    }

    const LOAD_V1: LoadDirTest = LoadDirTest::new("web")
        .has_v1()
        .expects_v1()
        .with_filename(DEFAULT_MICROFRONTENDS_CONFIG_V1);

    const LOAD_V1_ALT: LoadDirTest = LoadDirTest::new("web")
        .has_alt_v1()
        .expects_v1()
        .with_filename(DEFAULT_MICROFRONTENDS_CONFIG_V1_ALT);

    const LOAD_NONE: LoadDirTest = LoadDirTest::new("web");

    const LOAD_VERSIONLESS: LoadDirTest = LoadDirTest::new("web")
        .has_versionless()
        .expects_v1()
        .with_filename(DEFAULT_MICROFRONTENDS_CONFIG_V1);

    #[test_case(LOAD_V1)]
    #[test_case(LOAD_V1_ALT)]
    #[test_case(LOAD_NONE)]
    #[test_case(LOAD_VERSIONLESS)]
    fn test_load_dir(case: LoadDirTest) {
        let dir = TempDir::new().unwrap();
        let repo_root = AbsoluteSystemPath::new(dir.path().to_str().unwrap()).unwrap();
        let pkg_dir = AnchoredSystemPath::new(case.pkg_dir).unwrap();
        let pkg_path = repo_root.resolve(pkg_dir);
        if case.has_v1 {
            add_v1_config(&pkg_path).unwrap();
        }
        if case.has_alt_v1 {
            add_v1_alt_config(&pkg_path).unwrap();
        }
        if case.has_versionless {
            add_no_version_config(&pkg_path).unwrap();
        }

        let config = Config::load_from_dir(repo_root, pkg_dir).unwrap();
        let actual_version = config.as_ref().map(|config| match &config.inner {
            ConfigInner::V1(_) => FoundConfig::V1,
        });
        let actual_path = config.as_ref().and_then(|config| config.path());
        assert_eq!(actual_version, case.expected_version);
        assert_eq!(actual_path, case.expected_path().as_deref());
    }

    #[test]
    fn test_unsupported_version_from_dir() {
        let dir = TempDir::new().unwrap();
        let repo_root = AbsoluteSystemPath::new(dir.path().to_str().unwrap()).unwrap();
        let pkg_dir = AnchoredSystemPath::new("web").unwrap();
        let pkg_path = repo_root.resolve(pkg_dir);
        add_v2_config(&pkg_path).unwrap();
        let config = Config::load_from_dir(repo_root, pkg_dir);

        assert_matches!(config, Err(Error::UnsupportedVersion(..)));
    }
}
