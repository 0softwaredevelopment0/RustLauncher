//! The News screen: the feed from the configured site, card list and the
//! single-article detail view.

use crate::gui::state::App;
use crate::gui::theme::icons;
use crate::lang::{tr, tr_fmt};
use crate::services::news;
use crate::services::news::NewsItem;

impl App {
    pub(crate) fn ui_news(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        // Detail view: single article.
        if let Some(idx) = self.news_selected {
            let item = match &self.news {
                Some(Ok(items)) => items.get(idx).cloned(),
                _ => None,
            };
            if let Some(item) = item {
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{} {}", icons::ARROW_BACK, tr(lang, "Back")))
                        .clicked()
                    {
                        self.news_selected = None;
                    }
                });
                ui.add_space(4.0);
                self.news_detail(ui, &item);
                return;
            }
            self.news_selected = None;
        }

        // Cards stack vertically, one per row, and scroll downwards.
        ui.heading(tr(lang, "News"));
        ui.add_space(4.0);
        if ui.button(tr(lang, "Refresh")).clicked() {
            self.news = None;
            self.news_selected = None;
            self.fetch_news();
        }
        ui.separator();
        match &self.news {
            None => {
                ui.label(tr(lang, "Loading…"));
            }
            Some(Err(e)) => {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    tr_fmt(lang, "Feed unavailable: {0}", &[&e.to_string()]),
                );
                ui.add_space(4.0);
                let fallback = news::fallback_news(lang);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for item in &fallback {
                            self.news_card(ui, item, None);
                        }
                    });
            }
            Some(Ok(items)) => {
                let items = items.clone();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (idx, item) in items.iter().enumerate() {
                            self.news_card(ui, item, Some(idx));
                        }
                    });
            }
        }
    }

    fn news_card(&mut self, ui: &mut egui::Ui, item: &NewsItem, idx: Option<usize>) {
        let lang = self.settings.language;
        let card_bg = egui::Color32::from_rgba_premultiplied(30, 30, 40, 200);
        let border_color = egui::Color32::from_rgba_premultiplied(60, 60, 80, 200);

        let frame = egui::Frame::none()
            .fill(card_bg)
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(14.0, 12.0))
            .stroke(egui::Stroke::new(1.0_f32, border_color));

        let resp = frame
            .show(ui, |ui| {
                // Fill the full width so the single-column cards line up.
                ui.set_min_width(ui.available_width());
                // Top row: author + date.
                ui.horizontal(|ui| {
                    let name = item.author_name(lang);
                    ui.label(egui::RichText::new(name).small().strong());
                    ui.label(
                        egui::RichText::new(item.formatted_date())
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                });
                ui.add_space(6.0);
                // Title.
                ui.label(egui::RichText::new(&item.title).strong().size(15.0));
                ui.add_space(4.0);
                // Content preview (max 3 lines).
                let preview: String = item.content.lines().take(3).collect::<Vec<_>>().join("\n");
                ui.label(
                    egui::RichText::new(preview)
                        .small()
                        .color(egui::Color32::from_rgb(180, 180, 190)),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{}  {}",
                        icons::CHEVRON_RIGHT,
                        tr(lang, "Read more")
                    ))
                    .small()
                    .strong()
                    .color(egui::Color32::from_rgb(0, 200, 255)),
                );
            })
            .response;

        // Whole card is clickable — covers the title, preview and the
        // "Read more" label. This also makes fallback cards (news feed
        // unreachable) clickable, which previously did nothing.
        let idx_slot = idx.map(|i| i as u64).unwrap_or(u64::MAX);
        let card = ui.interact(
            resp.rect,
            egui::Id::new(("news_card", idx_slot)),
            egui::Sense::click(),
        );
        if card.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if card.clicked() && idx.is_some() {
            self.news_selected = idx;
        }
        ui.add_space(8.0);
    }

    fn news_detail(&self, ui: &mut egui::Ui, item: &NewsItem) {
        let lang = self.settings.language;
        // Author + date header.
        ui.horizontal(|ui| {
            let name = item.author_name(lang);
            ui.label(egui::RichText::new(name).strong().size(14.0));
            ui.separator();
            ui.label(egui::RichText::new(item.formatted_date()).color(egui::Color32::GRAY));
        });
        ui.add_space(8.0);
        // Title.
        ui.label(egui::RichText::new(&item.title).strong().size(20.0));
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        // Full content.
        for line in item.content.lines() {
            ui.label(line);
        }
        if item.content.is_empty() {
            ui.label(egui::RichText::new(tr(lang, "No content")).color(egui::Color32::GRAY));
        }
    }
}
