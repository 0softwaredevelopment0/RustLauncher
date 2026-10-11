//! The Accounts screen: three account kinds with their own creation and
//! removal proofs — Offline (nickname + password, Argon2id in the DB),
//! Ely.by (username + password against the Yggdrasil authserver) and
//! Mojang/Microsoft (OAuth device-code flow) — plus the removal dialog.

use anyhow::Result;

use crate::gui::state::{App, LauncherPaths, MsLoginState};
use crate::gui::toast_ui::draw_warning_triangle;
use crate::account::AccountStore;
use crate::account::{AccountKind, auth};
use crate::lang::{tr, tr_fmt};

impl App {
    /// The Accounts screen: three account kinds with their own creation and
    /// removal proofs — Offline (nickname + password, Argon2id in the DB),
    /// Ely.by (username + password against the Yggdrasil authserver) and
    /// Mojang/Microsoft (OAuth device-code flow).
    pub(crate) fn ui_accounts(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(tr(lang, "Accounts"));
        ui.add_space(4.0);

        // ── Add account ──────────────────────────────────────────
        ui.group(|ui| {
            ui.strong(tr(lang, "Add account"));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for kind in auth::AccountKind::ALL {
                    ui.selectable_value(&mut self.new_account_kind, *kind, kind.label(lang));
                }
            });
            match self.new_account_kind {
                AccountKind::Offline => {
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Nickname"));
                        let name = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.username_input)
                                    .hint_text(tr(lang, "3-16 chars"))
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        ui.label(tr(lang, "Password"));
                        let pass = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.new_account_password)
                                    .password(true)
                                    .hint_text(tr(lang, "required"))
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button(tr(lang, "Create")).clicked() || ((name || pass) && enter))
                            && !self.account_busy
                        {
                            let name = self.username_input.trim().to_string();
                            let password = self.new_account_password.clone();
                            self.account_busy = true;
                            self.spawn_job(
                                move || {
                                    // Argon2id hashing is intentionally slow;
                                    // keep it off the UI thread. The insert goes
                                    // through the DB so any store can do it; the
                                    // live in-memory store is refreshed afterwards.
                                    let mut store = AccountStore::load(&LauncherPaths::probe());
                                    store.add_offline(&name, &password, lang)
                                },
                                move |app, result| {
                                    app.account_busy = false;
                                    app.finish_account_change(result);
                                },
                            );
                        }
                    });
                    ui.weak(tr(
                        lang,
                        "The password is stored as an Argon2id hash in the launcher's database.",
                    ));
                }
                AccountKind::ElyBy => {
                    ui.horizontal(|ui| {
                        ui.label(tr(lang, "Ely.by email / login"));
                        let user = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.online_login_input)
                                    .hint_text(tr(lang, "you@example.com"))
                                    .desired_width(180.0),
                            )
                            .lost_focus();
                        ui.label(tr(lang, "Password"));
                        let pass = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.online_password_input)
                                    .password(true)
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button(tr(lang, "Log in")).clicked() || ((user || pass) && enter))
                            && !self.account_busy
                        {
                            let user = self.online_login_input.trim().to_string();
                            let password = self.online_password_input.clone();
                            self.account_busy = true;
                            self.spawn_job(
                                move || auth::login_elyby(&user, &password, lang),
                                move |app, result| {
                                    app.account_busy = false;
                                    app.finish_online_login(result);
                                },
                            );
                        }
                    });
                    ui.weak(tr(
                        lang,
                        "Logs in against authserver.ely.by; the session token is stored in the DB.",
                    ));
                }
                AccountKind::Mojang => {
                    ui.label(tr(
                        lang,
                        "Sign in with a Microsoft account that owns Minecraft: Java Edition.",
                    ));
                    if self.ms_login.is_none()
                        && !self.account_busy
                        && ui.button(tr(lang, "Start Microsoft sign-in")).clicked()
                    {
                        self.account_busy = true;
                        self.spawn_job(
                            move || {
                                auth::microsoft_begin(&crate::util::net::agent(), lang)
                                    .map_err(|e| e.to_string())
                            },
                            move |app, result| match result {
                                Ok((device_code, user_code, _expires, _interval)) => {
                                    app.account_busy = false;
                                    app.ms_login = Some(MsLoginState {
                                        device_code,
                                        user_code: user_code.clone(),
                                        error: None,
                                        cancelled: false,
                                    });
                                    let lang = app.settings.language;
                                    app.notify_info(tr_fmt(
                                        lang,
                                        "Enter the code {0} at microsoft.com/link",
                                        &[&user_code],
                                    ));
                                }
                                Err(e) => {
                                    app.account_busy = false;
                                    app.notify_error("MS-BEGIN", e);
                                }
                            },
                        );
                    }
                    let ms_state = self.ms_login.take();
                    if let Some(mut state) = ms_state {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(tr(lang, "Go to"));
                                ui.hyperlink("https://www.microsoft.com/link");
                                ui.add_space(8.0);
                                ui.label(tr(lang, "and enter code:"));
                                ui.label(
                                    egui::RichText::new(&state.user_code)
                                        .strong()
                                        .monospace()
                                        .size(18.0),
                                );
                                if ui.button(tr(lang, "Copy")).clicked() {
                                    ui.ctx().copy_text(state.user_code.clone());
                                }
                            });
                            ui.weak(tr(lang, "Waiting for you to finish in the browser…"));
                            if let Some(err) = &state.error {
                                ui.colored_label(egui::Color32::LIGHT_RED, err);
                            }
                            if ui.button(tr(lang, "Cancel")).clicked() {
                                state.cancelled = true;
                            }
                        });

                        if state.cancelled {
                            self.ms_login = None;
                        } else {
                            // Put the state back; the poll job below updates it.
                            self.ms_login = Some(state);

                            // Poll in the background; the result arrives via a job.
                            if !self.account_busy && !self.ms_polling {
                                let device_code = self
                                    .ms_login
                                    .as_ref()
                                    .map(|s| s.device_code.clone())
                                    .unwrap_or_default();
                                self.ms_polling = true;
                                self.spawn_job(
                                    move || {
                                        auth::microsoft_poll(
                                            &crate::util::net::agent(),
                                            &device_code,
                                            lang,
                                        )
                                        .map_err(|e| e.to_string())
                                    },
                                    move |app, result| {
                                        app.ms_polling = false;
                                        match result {
                                            Ok(Some(login)) => {
                                                app.ms_login = None;
                                                app.ms_polling = false;
                                                // Add to the live store directly: the
                                                // HTTP work is done, the insert is a
                                                // fast local SQLite write.
                                                let result = app.accounts.add_mojang(
                                                    &login.username,
                                                    &login.uuid,
                                                    &login.access_token,
                                                    &login.refresh_token,
                                                );
                                                app.finish_account_change(result);
                                            }
                                            Ok(None) => {} // still waiting; poll again next frames
                                            Err(e) => {
                                                if let Some(state) = &mut app.ms_login {
                                                    state.error = Some(e);
                                                }
                                            }
                                        }
                                    },
                                );
                            }
                        }
                    }
                }
            }
            if self.account_busy {
                ui.spinner();
                ui.weak(tr(lang, "working…"));
            }
            if let Some(error) = &self.account_error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });

        ui.add_space(6.0);

        // ── Account list ─────────────────────────────────────────
        ui.strong(tr(lang, "Your accounts"));
        ui.add_space(2.0);
        let rows: Vec<(AccountKind, String)> = self
            .accounts
            .accounts
            .iter()
            .map(|a| (a.kind, a.username.clone()))
            .collect();
        for (kind, name) in &rows {
            ui.horizontal(|ui| {
                let selected = self.accounts.current.as_deref() == Some(name.as_str());
                if ui.radio(selected, name).clicked() {
                    self.accounts.select(name);
                    self.save_accounts();
                }
                ui.weak(kind.label(lang));
                if ui.small_button(tr(lang, "Remove")).clicked() {
                    // Removal always asks for proof: the password for offline
                    // accounts, a fresh login for online ones.
                    self.account_remove_pending = Some(name.clone());
                    self.account_remove_password.clear();
                    self.account_remove_error = None;
                }
            });
        }
        if rows.is_empty() {
            ui.weak(tr(lang, "No accounts yet — create one above."));
        }
    }

    /// Common tail of every successful account mutation: refresh selection
    /// state, clear inputs, drop the error.
    pub(crate) fn finish_account_change(&mut self, result: Result<String>) {
        let lang = self.settings.language;
        match result {
            Ok(name) => {
                self.username_input.clear();
                self.new_account_password.clear();
                self.account_error = None;
                // Background jobs add the account to a throwaway store loaded
                // from the DB; reload the in-memory copy so the new account
                // actually shows up in the list and can be selected.
                self.accounts = AccountStore::load(&self.home_dir);
                self.accounts.select(&name);
                self.save_accounts();
                self.notify_info(tr_fmt(lang, "Account {0} added", &[&name]));
            }
            Err(e) => self.account_error = Some(e.to_string()),
        }
    }

    /// Store an online login result (Ely.by or Microsoft) as an account.
    /// Runs on the UI thread: the HTTP work already happened in the job; the
    /// store write is a fast local SQLite insert.
    pub(crate) fn finish_online_login(&mut self, result: Result<auth::Account>) {
        match result {
            Ok(account) => {
                self.online_password_input.clear();
                // Mutate the live store directly so the new account is in the
                // list immediately (and persisted by the store itself).
                let result = match account.kind {
                    AccountKind::ElyBy => self.accounts.add_elyby(
                        &account.username,
                        &account.uuid,
                        &account.access_token,
                    ),
                    AccountKind::Mojang => self.accounts.add_mojang(
                        &account.username,
                        &account.uuid,
                        &account.access_token,
                        "",
                    ),
                    AccountKind::Offline => unreachable!(),
                };
                self.finish_account_change(result);
            }
            Err(e) => {
                self.account_error = Some(e.to_string());
            }
        }
    }

    /// The removal dialog: password proof for offline accounts, re-login for
    /// online ones. Mirrors the Stop/Kill confirmation styling.
    pub(crate) fn show_account_removal(&mut self, ctx: &egui::Context) {
        let Some(name) = self.account_remove_pending.clone() else {
            return;
        };
        let rec = self.accounts.accounts.iter().find(|a| a.username == name);
        let kind = rec.map(|r| r.kind);
        let screen = ctx.screen_rect();
        let lang = self.settings.language;

        egui::Area::new(egui::Id::new("account_remove_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                if resp.clicked() {
                    self.account_remove_pending = None;
                }
            });

        egui::Window::new(egui::RichText::new(tr(lang, "Confirm removal")).strong())
            .id(egui::Id::new("account_remove_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            // Hard clamp: min = max = default, so the dialog physically
            // cannot stretch regardless of what egui remembers or measures.
            .fixed_size(egui::vec2(400.0, 230.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    draw_warning_triangle(ui, 36.0);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(tr_fmt(lang, "Remove {0}?", &[&name]))
                                    .strong()
                                    .size(16.0),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        match kind {
                            Some(AccountKind::Offline) => {
                                ui.label(tr(
                                    lang,
                                    "Enter the account password to confirm removal.",
                                ));
                            }
                            Some(AccountKind::ElyBy) => {
                                ui.label(tr(lang, "Sign in to Ely.by again to confirm removal."));
                            }
                            Some(AccountKind::Mojang) => {
                                ui.label(tr(
                                    lang,
                                    "Sign in with Microsoft again to confirm removal.",
                                ));
                            }
                            None => {}
                        }
                    });
                });
                ui.add_space(10.0);

                let confirmed = match kind {
                    Some(AccountKind::Offline) => {
                        ui.horizontal(|ui| {
                            ui.label(tr(lang, "Password:"));
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.account_remove_password)
                                    .password(true)
                                    .hint_text(tr(lang, "type the account password"))
                                    .desired_width(240.0)
                                    .id(egui::Id::new("account_remove_password")),
                            );
                            // Auto-focus the password field while nothing else
                            // in the dialog holds focus, so it is obvious where
                            // to type right after the dialog opens.
                            if ui.memory(|m| m.focused().is_none()) {
                                ui.memory_mut(|m| {
                                    m.request_focus(egui::Id::new("account_remove_password"))
                                });
                            }
                            let enter =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            enter
                                && self.accounts.verify_offline_password(
                                    &name,
                                    self.account_remove_password.trim(),
                                )
                        });
                        if let Some(err) = &self.account_remove_error {
                            ui.colored_label(egui::Color32::LIGHT_RED, err);
                        }
                        let ok = !self.account_remove_password.trim().is_empty()
                            && self.accounts.verify_offline_password(
                                &name,
                                self.account_remove_password.trim(),
                            );
                        let mut clicked = false;
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            clicked = ui
                                .add_enabled(
                                    ok,
                                    egui::Button::new(
                                        egui::RichText::new(tr(lang, "Confirm")).strong(),
                                    ),
                                )
                                .clicked();
                        });
                        ok && clicked
                    }
                    Some(AccountKind::ElyBy) => {
                        ui.horizontal(|ui| {
                            ui.label(tr(lang, "Ely.by login:"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.account_remove_login)
                                    .desired_width(220.0),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(tr(lang, "Password:"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.account_remove_password)
                                    .password(true)
                                    .desired_width(220.0),
                            );
                        });
                        if let Some(err) = &self.account_remove_error {
                            ui.colored_label(egui::Color32::LIGHT_RED, err);
                        }
                        let ready = !self.account_remove_login.trim().is_empty()
                            && !self.account_remove_password.is_empty();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    ready,
                                    egui::Button::new(
                                        egui::RichText::new(tr(lang, "Confirm")).strong(),
                                    ),
                                )
                                .clicked()
                            {
                                self.account_remove_error = None;
                                let user = self.account_remove_login.trim().to_string();
                                let password = self.account_remove_password.clone();
                                let name = name.clone();
                                self.spawn_job(
                                    move || auth::login_elyby(&user, &password, lang),
                                    move |app, res| match res {
                                        Ok(acc) => {
                                            if acc.username == name {
                                                app.remove_account_confirmed(&name);
                                            } else {
                                                let lang = app.settings.language;
                                                app.account_remove_error = Some(
                                                    tr(
                                                        lang,
                                                        "that login belongs to another account",
                                                    )
                                                    .into(),
                                                );
                                            }
                                        }
                                        Err(e) => app.account_remove_error = Some(e.to_string()),
                                    },
                                );
                            }
                        });
                        false
                    }
                    Some(AccountKind::Mojang) => {
                        ui.label(tr(
                    lang,
                    "A Microsoft sign-in window will open. Complete it to remove this account.",
                ));
                        if ui.button(tr(lang, "Start Microsoft sign-in")).clicked() {
                            let name = name.clone();
                            self.account_busy = true;
                            self.spawn_job(
                                move || {
                                    auth::microsoft_begin(&crate::util::net::agent(), lang)
                                        .map_err(|e| e.to_string())
                                },
                                move |app, result| match result {
                                    Ok((device_code, user_code, _, _)) => {
                                        app.account_busy = false;
                                        app.account_remove_pending = Some(name.clone());
                                        app.ms_removal = Some(MsLoginState {
                                            device_code,
                                            user_code: user_code.clone(),
                                            error: None,
                                            cancelled: false,
                                        });
                                        let lang = app.settings.language;
                                        app.notify_info(tr_fmt(
                                            lang,
                                            "Enter the code {0} at microsoft.com/link",
                                            &[&user_code],
                                        ));
                                    }
                                    Err(e) => {
                                        app.account_busy = false;
                                        app.account_remove_error = Some(e);
                                    }
                                },
                            );
                        }
                        if let Some(err) = &self.account_remove_error {
                            ui.colored_label(egui::Color32::LIGHT_RED, err);
                        }
                        false
                    }
                    None => false,
                };
                if confirmed {
                    self.remove_account_confirmed(&name);
                }

                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        self.account_remove_pending = None;
                        self.ms_removal = None;
                    }
                });
            });

        // The Microsoft re-login for removal shares the device-code poller.
        if self.ms_removal.is_some() && !self.account_busy && !self.ms_polling {
            let device_code = self
                .ms_removal
                .as_ref()
                .map(|s| s.device_code.clone())
                .unwrap_or_default();
            let name = name.clone();
            self.ms_polling = true;
            self.spawn_job(
                move || {
                    auth::microsoft_poll(&crate::util::net::agent(), &device_code, lang)
                        .map_err(|e| e.to_string())
                },
                move |app, result| {
                    app.ms_polling = false;
                    match result {
                        Ok(Some(login)) => {
                            if login.username == name {
                                app.ms_removal = None;
                                app.remove_account_confirmed(&name);
                            } else {
                                if let Some(state) = &mut app.ms_removal {
                                    let lang = app.settings.language;
                                    state.error = Some(
                                        tr(lang, "that Microsoft account is not this account")
                                            .into(),
                                    );
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if let Some(state) = &mut app.ms_removal {
                                state.error = Some(e);
                            }
                        }
                    }
                },
            );
        }
    }

    /// Remove the account (proof already collected). Falls back to the first
    /// remaining account when the removed one was selected.
    pub(crate) fn remove_account_confirmed(&mut self, name: &str) {
        let lang = self.settings.language;
        let _ = self.accounts.remove(name);
        // Re-sync with the DB so the in-memory list matches what is stored.
        self.accounts = AccountStore::load(&self.home_dir);
        self.account_remove_pending = None;
        self.account_remove_password.clear();
        self.account_remove_login.clear();
        self.account_remove_error = None;
        self.save_accounts();
        self.notify_info(tr_fmt(lang, "Account {0} removed", &[name]));
    }
}
