//! Persistence and refresh helpers: saving settings/accounts/servers,
//! rescanning versions and skins, and fetching the news feed.

use super::super::state::App;
use crate::util::home;
use crate::game::version;
use crate::services::news;
use crate::services::skins;

impl App {
    pub(crate) fn save_settings(&self) {
        let _ = self.settings.save(&self.home_dir);
    }

    /// Persist the current account selection (the store itself writes to the
    /// SQLite DB on every mutation; only the selected name lives in settings).
    pub(crate) fn save_accounts(&mut self) {
        self.settings.username = self
            .accounts
            .current()
            .map(|a| a.username.clone())
            .unwrap_or_default();
        self.save_settings();
    }

    pub(crate) fn save_servers(&mut self) {
        self.versions = version::list_versions(&self.active_game_dir()).unwrap_or_default();
        let _ = self.servers.save(&home::servers_file(&self.home_dir));
    }

    pub(crate) fn reload_versions(&mut self) {
        self.versions = version::list_versions(&self.active_game_dir()).unwrap_or_default();
    }

    pub(crate) fn refresh_skins(&mut self) {
        self.skin_names = skins::list_skins(&home::skins_dir(&self.home_dir))
            .iter()
            .filter_map(|p| p.file_stem().and_then(|n| n.to_str()))
            .map(str::to_string)
            .collect();
    }

    pub(crate) fn select_saved_skin(&mut self) {
        if self.selected_skin.is_none() {
            // Show the skin of the current account, if downloaded.
            if let Some(account) = self.accounts.current() {
                let path =
                    home::skins_dir(&self.home_dir).join(format!("{}.png", account.username));
                if path.is_file() {
                    self.selected_skin = Some(account.username.clone());
                }
            }
        }
    }

    pub(crate) fn fetch_news(&mut self) {
        let mut base = self
            .settings
            .news_url
            .trim()
            .trim_end_matches('/')
            .to_string();
        // The setting is the site base URL, but tolerate users who pasted the
        // news page address (`…/news`) — the API lives at the site root.
        if base.ends_with("/news") {
            base.truncate(base.len() - "/news".len());
        }
        let url = format!("{base}/api/news");
        let lang = self.settings.language;
        self.spawn_job(
            move || {
                let agent = crate::util::net::agent();
                news::fetch(&agent, &url, lang)
            },
            |app, result| {
                app.news = Some(result.map_err(|e| e.to_string()));
            },
        );
    }
}
