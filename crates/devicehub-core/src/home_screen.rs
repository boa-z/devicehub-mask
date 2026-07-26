//! Bounded, host-independent home-screen data returned to adapters.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperKind {
    Home,
    Lock,
}

impl WallpaperKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "home" => Some(Self::Home),
            "lock" => Some(Self::Lock),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "home screen",
            Self::Lock => "lock screen",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeScreenContainer {
    Dock,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HomeScreenFolderStep {
    pub name: Option<String>,
    pub page: u16,
    pub position: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HomeScreenAppLocation {
    pub bundle_id: String,
    pub name: Option<String>,
    pub container: HomeScreenContainer,
    pub page: Option<u16>,
    pub position: u16,
    pub folders: Vec<HomeScreenFolderStep>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HomeScreenIconMetrics {
    pub screen_width: Option<u32>,
    pub screen_height: Option<u32>,
    pub icon_width: Option<u32>,
    pub icon_height: Option<u32>,
    pub columns: Option<u16>,
    pub rows: Option<u16>,
    pub dock_max_count: Option<u16>,
    pub folder_columns: Option<u16>,
    pub folder_rows: Option<u16>,
    pub max_pages: Option<u16>,
    pub folder_max_pages: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HomeScreenLayout {
    pub apps: Vec<HomeScreenAppLocation>,
    pub page_count: u16,
    pub metrics: Option<HomeScreenIconMetrics>,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpaper_kind_accepts_only_public_route_values() {
        assert_eq!(WallpaperKind::parse("home"), Some(WallpaperKind::Home));
        assert_eq!(WallpaperKind::parse("lock"), Some(WallpaperKind::Lock));
        assert_eq!(WallpaperKind::parse("homescreen"), None);
    }
}
