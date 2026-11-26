use egui::{Align, Color32, CornerRadius, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::engine::controller::Controller;
use crate::engine::views::{
    ReviewBoardView, ReviewGameView, ReviewListView, ReviewOptionView, ReviewSummaryView,
};
use crate::images::ImageCache;
use crate::ui::theme::{
    ACCENT, BG, BG2, DIM, LINE, LINE2, MUTED, NEG, PANEL, POS, RAISED, TEXT, TEXT2, fmt_signed, heat_bg,
    short_lane, strong, strong_font,
};

/// Which screen the idle (no champ select) panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdleView {
    #[default]
    Blind,
    Review,
}

/// "For you" badges saturate at this many points, like the website review.
const HEAT_MAX: f64 = 4.0;
const LIST_ROW_H: f32 = 52.0;
const TABLE_ROW_H: f32 = 26.0;
const NUM_COL_W: f32 = 62.0;

enum ReviewMsg {
    List(Result<ReviewListView, String>),
    BackfillDone,
}

pub struct ReviewUiState {
    pub list: Option<ReviewListView>,
    pub error: Option<String>,
    /// Older drafts are being rebuilt from saved snapshots in the background.
    pub backfilling: bool,
    pub selected: Option<i64>,
    tx: mpsc::UnboundedSender<ReviewMsg>,
    rx: mpsc::UnboundedReceiver<ReviewMsg>,
    started: bool,
    /// Account the older drafts were last rebuilt for; a new account gets its own rebuild.
    backfilled_for: Option<Option<String>>,
    last_fetch: Option<Instant>,
}

impl Default for ReviewUiState {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            list: None,
            error: None,
            backfilling: false,
            selected: None,
            tx,
            rx,
            started: false,
            backfilled_for: None,
            last_fetch: None,
        }
    }
}

impl ReviewUiState {
    /// Read the store and, the first time only, rebuild reviews for older drafts first. Both run
    /// on the runtime, never in the frame.
    fn trigger_load(&mut self, ctl: &Arc<Controller>, ctx: &egui::Context, backfill: bool) {
        self.error = None;
        self.last_fetch = Some(Instant::now());
        self.backfilling = backfill;
        let (ctl_c, tx, ctx_c) = (Arc::clone(ctl), self.tx.clone(), ctx.clone());
        tokio::spawn(async move {
            let _ = tx.send(ReviewMsg::List(ctl_c.review_list().await));
            ctx_c.request_repaint();
            if backfill {
                if ctl_c.backfill_reviews().await > 0 {
                    let _ = tx.send(ReviewMsg::List(ctl_c.review_list().await));
                }
                let _ = tx.send(ReviewMsg::BackfillDone);
                ctx_c.request_repaint();
            }
        });
    }

    /// Call when the view is opened: the first time for each account it also backfills; later
    /// it just refreshes.
    pub fn activate(&mut self, ctl: &Arc<Controller>, ctx: &egui::Context) {
        self.started = true;
        let account = ctl.store.as_ref().and_then(|s| s.account());
        let backfill = self.backfilled_for.as_ref() != Some(&account);
        self.backfilled_for = Some(account);
        self.trigger_load(ctl, ctx, backfill);
    }

    fn poll(&mut self, ctl: &Arc<Controller>, ctx: &egui::Context) {
        if !self.started {
            self.activate(ctl, ctx);
        }
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                ReviewMsg::List(Ok(list)) => {
                    self.error = None;
                    if self
                        .selected
                        .is_none_or(|s| !list.games.iter().any(|g| g.draft_id == s))
                    {
                        self.selected = list.games.first().map(|g| g.draft_id);
                    }
                    self.list = Some(list);
                }
                ReviewMsg::List(Err(e)) => self.error = Some(e),
                ReviewMsg::BackfillDone => self.backfilling = false,
            }
        }
    }

    /// True once there is something to show and nothing is still being rebuilt.
    pub fn settled(&self) -> bool {
        self.started && !self.backfilling && (self.list.is_some() || self.error.is_some())
    }
}

/// Segmented toggle between the blind-pick chart and the draft review (same look as the role pill).
pub fn idle_toggle(ui: &mut Ui, view: &mut IdleView) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(BG2)
        .stroke(Stroke::new(1.0, LINE2))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::symmetric(4, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (v, label) in [(IdleView::Blind, "Blind picks"), (IdleView::Review, "Draft review")] {
                    let on = *view == v;
                    let btn = egui::Button::new(
                        RichText::new(label).size(11.5).strong().color(if on { BG } else { MUTED }),
                    )
                    .fill(if on { ACCENT } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(88.0, 22.0))
                    .corner_radius(CornerRadius::same(13));
                    if ui.add(btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
                        *view = v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

fn signed_color(v: f64) -> Color32 {
    if v.abs() < 0.05 {
        DIM
    } else if v > 0.0 {
        POS
    } else {
        NEG
    }
}

fn role_name(r: &str) -> &'static str {
    match short_lane(r) {
        "TOP" => "Top",
        "JG" => "Jungle",
        "MID" => "Mid",
        "BOT" => "Bot",
        "SUP" => "Support",
        _ => "",
    }
}

fn result_text(g: &ReviewGameView) -> (&'static str, Color32) {
    match g.result {
        Some(true) => ("Win", POS),
        Some(false) => ("Loss", NEG),
        None => ("No result yet", DIM),
    }
}

/// Heat-coloured pill with a signed value, painted inline.
/// Fixed-size score pill, so every badge in a column (and across views) has the same
/// width and height regardless of the digits or the surrounding layout.
const BADGE_SIZE: Vec2 = Vec2::new(42.0, 18.0);

fn score_badge(ui: &mut Ui, v: f64) -> egui::Response {
    let (bg, fg) = heat_bg(v, HEAT_MAX);
    let galley = ui.painter().layout_no_wrap(fmt_signed(v, 1), strong_font(12.0), fg);
    // Grow only if the text truly cannot fit (e.g. "+12.5"), never shrink.
    let size = Vec2::new(BADGE_SIZE.x.max(galley.size().x + 10.0), BADGE_SIZE.y);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(4), bg);
        p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    }
    resp
}

fn chip(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(9.5).strong().color(color));
        });
}

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn caps(text: &str) -> RichText {
    strong(text.to_uppercase()).size(9.5).color(MUTED)
}

pub fn render_review(ui: &mut Ui, ctl: &Arc<Controller>, st: &mut ReviewUiState, imgs: &mut ImageCache) {
    st.poll(ctl, ui.ctx());

    let Some(list) = st.list.clone() else {
        ui.centered_and_justified(|ui| {
            if let Some(err) = st.error.clone() {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(err).color(NEG));
                    if ui.button("Retry").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        st.trigger_load(ctl, ui.ctx(), true);
                    }
                });
            } else {
                let msg = if st.backfilling {
                    "Rebuilding past drafts from saved snapshots..."
                } else {
                    "Loading your drafts..."
                };
                ui.label(RichText::new(msg).color(MUTED));
            }
        });
        return;
    };

    if list.games.is_empty() {
        ui.centered_and_justified(|ui| {
            let msg = if st.backfilling {
                "Rebuilding past drafts from saved snapshots..."
            } else {
                "No drafts recorded yet. Draftly saves the board each time you lock in a pick."
            };
            ui.label(RichText::new(msg).size(13.0).color(MUTED));
        });
        return;
    }

    render_summary(ui, &list.summary);
    ui.add_space(8.0);

    let avail_w = ui.available_width();
    let avail_h = ui.available_height().max(120.0);
    let list_w = (avail_w * 0.34).clamp(210.0, 300.0).min(avail_w * 0.5);
    let (area, _) = ui.allocate_exact_size(Vec2::new(avail_w, avail_h), Sense::hover());
    let left = Rect::from_min_size(area.min, Vec2::new(list_w, avail_h));
    let right = Rect::from_min_max(Pos2::new(left.right() + 12.0, area.top()), area.max);

    let mut left_ui = ui.new_child(egui::UiBuilder::new().id_salt("review_list").max_rect(left));
    left_ui.set_clip_rect(left.intersect(ui.clip_rect()));
    render_game_list(&mut left_ui, left, &list, &mut st.selected, imgs);

    let mut right_ui = ui.new_child(egui::UiBuilder::new().id_salt("review_detail").max_rect(right));
    right_ui.set_clip_rect(right.intersect(ui.clip_rect()));
    if let Some(game) = list
        .games
        .iter()
        .find(|g| Some(g.draft_id) == st.selected)
        .or_else(|| list.games.first())
    {
        egui::ScrollArea::vertical()
            .id_salt(("review_detail_scroll", game.draft_id))
            .auto_shrink([false, false])
            .show(&mut right_ui, |ui| {
                render_detail(ui, game, list.cap, imgs);
            });
    }
}

fn render_summary(ui: &mut Ui, s: &ReviewSummaryView) {
    let (best, avg, record) = if s.scored > 0 {
        (
            format!("{} of {}", s.best_taken, s.scored),
            format!("{:.1}", s.avg_left),
            format!("{}-{}", s.wins, s.losses),
        )
    } else {
        ("-".to_string(), "-".to_string(), format!("{}-{}", s.wins, s.losses))
    };
    let stats = [
        (best, "games where you took your best available pick"),
        (avg, "average score left on the table"),
        (record, "W-L in reviewed games"),
    ];
    ui.columns(3, |cols| {
        for (col, (value, caption)) in cols.iter_mut().zip(stats) {
            card(col, |ui| {
                ui.label(strong(value).size(17.0).color(TEXT));
                ui.add(egui::Label::new(RichText::new(caption).size(10.5).color(MUTED)).truncate());
            });
        }
    });
}

fn render_game_list(ui: &mut Ui, rect: Rect, list: &ReviewListView, selected: &mut Option<i64>, imgs: &mut ImageCache) {
    ui.painter().rect_filled(rect, CornerRadius::same(8), PANEL);
    ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    let inner = rect.shrink(6.0);
    let mut list_ui = ui.new_child(egui::UiBuilder::new().id_salt("review_list_rows").max_rect(inner));
    list_ui.set_clip_rect(inner.intersect(ui.clip_rect()));
    list_ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(&mut list_ui, LIST_ROW_H, list.games.len(), |ui, range| {
            for i in range {
                let g = &list.games[i];
                let (row, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), LIST_ROW_H), Sense::click());
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                let on = *selected == Some(g.draft_id);
                if resp.clicked() {
                    *selected = Some(g.draft_id);
                }
                if on || resp.hovered() {
                    let fill = if on {
                        Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 22)
                    } else {
                        RAISED
                    };
                    ui.painter().rect_filled(row.shrink(1.0), CornerRadius::same(4), fill);
                }
                if on {
                    ui.painter().rect_stroke(
                        row.shrink(1.0),
                        CornerRadius::same(4),
                        Stroke::new(1.0, ACCENT.gamma_multiply(0.55)),
                        egui::StrokeKind::Inside,
                    );
                }
                let icon = Rect::from_min_size(Pos2::new(row.left() + 6.0, row.center().y - 16.0), Vec2::splat(32.0));
                let mut icon_ui = ui.new_child(egui::UiBuilder::new().max_rect(icon));
                imgs.paint_icon(&mut icon_ui, &g.champ.name, &g.champ.icon, icon.size(), 5.0, Color32::WHITE);

                // Right edge: score badge, vertically centered so every row's score lines up.
                // The "Best" marker sits inline on the name line instead.
                let badge_w = 52.0;
                let badge_rect = Rect::from_min_max(
                    Pos2::new(row.right() - badge_w - 6.0, row.top() + 4.0),
                    Pos2::new(row.right() - 6.0, row.bottom() - 4.0),
                );
                let mut badge_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(badge_rect)
                        .layout(Layout::right_to_left(Align::Center)),
                );
                match &g.picked {
                    Some(p) => {
                        score_badge(&mut badge_ui, p.score).on_hover_text("Score of your pick, for you");
                    }
                    None => {
                        badge_ui.label(RichText::new("-").color(DIM)).on_hover_text("No score for this pick");
                    }
                }

                let text_left = icon.right() + 8.0;
                let text_rect = Rect::from_min_max(
                    Pos2::new(text_left, row.top()),
                    Pos2::new(badge_rect.left() - 4.0, row.bottom()),
                );
                let p = ui.painter_at(text_rect);
                let (res_txt, res_col) = result_text(g);
                let name_galley = p.layout_no_wrap(g.champ.name.clone(), strong_font(13.0), if on { ACCENT } else { TEXT });
                let name_w = name_galley.size().x;
                p.galley(Pos2::new(text_left, row.center().y - 15.0), name_galley, TEXT);
                let (res_pos, res_font) = if g.result.is_some() {
                    (Pos2::new(text_left + name_w + 6.0, row.center().y - 14.0), strong_font(12.0))
                } else {
                    (Pos2::new(text_left + name_w + 6.0, row.center().y - 13.0), egui::FontId::proportional(10.0))
                };
                let short = match g.result {
                    Some(true) => "W",
                    Some(false) => "L",
                    None => res_txt,
                };
                let res_rect = p.text(res_pos, egui::Align2::LEFT_TOP, short, res_font, res_col);
                if g.took_best && g.picked.is_some() {
                    let best_galley = p.layout_no_wrap("Best".to_owned(), strong_font(9.5), ACCENT);
                    let chip_rect = Rect::from_min_size(
                        Pos2::new(res_rect.right() + 6.0, row.center().y - 14.0),
                        best_galley.size() + Vec2::new(10.0, 2.0),
                    );
                    p.rect_stroke(
                        chip_rect,
                        CornerRadius::same(8),
                        Stroke::new(1.0, ACCENT.gamma_multiply(0.6)),
                        egui::StrokeKind::Inside,
                    );
                    p.galley(chip_rect.min + Vec2::new(5.0, 1.0), best_galley, ACCENT);
                }
                let mut sub = format!("{} · {}", role_name(&g.role), g.date);
                if g.replayed {
                    sub.push_str(" · rebuilt");
                }
                p.text(
                    Pos2::new(text_left, row.center().y + 3.0),
                    egui::Align2::LEFT_TOP,
                    sub,
                    egui::FontId::proportional(10.5),
                    MUTED,
                );
            }
        });
}

fn note_for(o: &ReviewOptionView) -> String {
    match (o.games, o.wins) {
        (Some(g), Some(w)) if g > 0 => {
            format!("{g} {}, {}% WR", if g == 1 { "game" } else { "games" }, (100.0 * f64::from(w) / f64::from(g)).round())
        }
        _ => match o.mastery {
            Some(m) => format!("No games on record, {m} mastery points"),
            None => "No games on record".to_string(),
        },
    }
}

fn choice_card(ui: &mut Ui, label: &str, o: Option<&ReviewOptionView>, value: Option<f64>, note: String, fallback: Option<(&str, &str)>, imgs: &mut ImageCache) {
    card(ui, |ui| {
        ui.label(caps(label));
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if let Some(o) = o {
                imgs.paint_icon(ui, &o.champ.name, &o.champ.icon, Vec2::splat(30.0), 5.0, Color32::WHITE);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(egui::Label::new(strong(&o.champ.name).size(13.0).color(TEXT)).truncate());
                    if let Some(v) = value {
                        score_badge(ui, v);
                    }
                });
            } else if let Some((name, icon)) = fallback {
                imgs.paint_icon(ui, name, icon, Vec2::splat(30.0), 5.0, Color32::WHITE);
                ui.add(egui::Label::new(strong(name).size(13.0).color(TEXT)).truncate());
            }
        });
        ui.add(egui::Label::new(RichText::new(note).size(10.5).color(MUTED)).wrap());
    });
}

fn icon_row(ui: &mut Ui, items: &[(&crate::engine::views::ChampRef, bool, String)], empty: &str, imgs: &mut ImageCache) {
    if items.is_empty() {
        ui.label(RichText::new(empty).size(11.0).color(DIM));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        for (c, solid, tip) in items {
            let tint = if *solid { Color32::WHITE } else { Color32::from_rgba_unmultiplied(255, 255, 255, 110) };
            imgs.paint_icon(ui, &c.name, &c.icon, Vec2::splat(26.0), 4.0, tint).on_hover_text(tip);
        }
    });
}

fn render_board(ui: &mut Ui, b: &ReviewBoardView, imgs: &mut ImageCache) {
    card(ui, |ui| {
        ui.label(caps("Board when you picked"));
        ui.add_space(3.0);
        ui.columns(2, |cols| {
            cols[0].label(RichText::new("Teammates locked").size(10.5).color(MUTED));
            let allies: Vec<_> = b
                .allies
                .iter()
                .map(|a| {
                    let tip = if a.locked {
                        format!("{} · {}", a.champ.name, role_name(&a.position))
                    } else {
                        format!("{} · hovering", a.champ.name)
                    };
                    (&a.champ, a.locked, tip)
                })
                .collect();
            icon_row(&mut cols[0], &allies, "None yet", imgs);
            cols[1].label(RichText::new("Enemies showing").size(10.5).color(MUTED));
            let enemies: Vec<_> = b
                .enemies
                .iter()
                .map(|e| {
                    let tip = match &e.lane {
                        Some(l) => format!("{} · {}", e.champ.name, role_name(l)),
                        None => e.champ.name.clone(),
                    };
                    (&e.champ, true, tip)
                })
                .collect();
            icon_row(&mut cols[1], &enemies, "None yet", imgs);
        });
        ui.add_space(3.0);
        let text = if b.blind {
            "Scored as a blind pick: your lane opponent was not showing yet."
        } else {
            "Scored as a counter pick: your lane opponent was already showing."
        };
        ui.label(RichText::new(text).size(11.0).color(TEXT2));
    });
}

fn render_detail(ui: &mut Ui, g: &ReviewGameView, cap: f64, imgs: &mut ImageCache) {
    ui.spacing_mut().item_spacing.y = 8.0;
    let (res_txt, res_col) = result_text(g);
    ui.horizontal_wrapped(|ui| {
        ui.label(strong(format!("{} · {} · ", role_name(&g.role), g.date)).size(15.0).color(TEXT));
        ui.label(strong(res_txt).size(15.0).color(res_col));
    });

    let Some(board) = &g.board else {
        ui.label(RichText::new(&g.verdict).size(12.0).color(MUTED));
        return;
    };
    render_board(ui, board, imgs);

    let champ_fallback = (g.champ.name.as_str(), g.champ.icon.as_str());
    ui.columns(3, |cols| {
        let picked_note = match &g.picked {
            Some(p) => note_for(p),
            None => "No data in this role at this tier.".to_string(),
        };
        choice_card(&mut cols[0], "You picked", g.picked.as_ref(), g.picked.as_ref().map(|p| p.score), picked_note, Some(champ_fallback), imgs);
        if let Some(b) = &g.best_you {
            let same = g.picked.as_ref().is_some_and(|p| p.champ.id == b.champ.id);
            let note = if same { "Same as your pick".to_string() } else { note_for(b) };
            choice_card(&mut cols[1], "Best for you", Some(b), Some(b.score), note, None, imgs);
        }
        if let Some(b) = &g.best_anyone {
            let mut note = note_for(b);
            if b.comfort <= -0.05 {
                note = format!("{note}; comfort {} for you", fmt_signed(b.comfort, 1));
            }
            choice_card(&mut cols[2], "Best for anyone", Some(b), Some(b.anyone), note, None, imgs);
        }
    });

    ui.label(strong(&g.verdict).size(13.0).color(TEXT));
    if g.replayed {
        ui.label(
            RichText::new("Rebuilt from the saved draft using today's champion statistics.")
                .size(10.5)
                .italics()
                .color(MUTED),
        );
    }
    ui.label(
        RichText::new(format!(
            "\"For you\" includes a personal bonus from your history and mastery, capped at +{cap:.1}."
        ))
        .size(10.5)
        .color(MUTED),
    );

    render_table(ui, g, imgs);
}

fn render_table(ui: &mut Ui, g: &ReviewGameView, imgs: &mut ImageCache) {
    let total_w = ui.available_width();
    let gap = 6.0;
    let champ_w = (total_w * 0.28).clamp(84.0, 150.0);
    let tags_w = (total_w - champ_w - 3.0 * NUM_COL_W - 4.0 * gap).max(56.0);
    let cell = |ui: &mut Ui, w: f32, layout: Layout, add: &mut dyn FnMut(&mut Ui)| {
        ui.allocate_ui_with_layout(Vec2::new(w, TABLE_ROW_H), layout, |ui| {
            ui.set_width(w);
            add(ui);
        });
    };
    let lc = Layout::left_to_right(Align::Center);
    let rc = Layout::right_to_left(Align::Center);

    card(ui, |ui| {
        egui::Grid::new(("review_table", g.draft_id))
            .num_columns(5)
            .spacing([gap, 2.0])
            .show(ui, |ui| {
                for (w, text, right) in [
                    (champ_w, "Champion", false),
                    (NUM_COL_W, "For anyone", true),
                    (NUM_COL_W, "Comfort", true),
                    (NUM_COL_W, "For you", true),
                    (tags_w, "Tags", false),
                ] {
                    cell(ui, w, if right { rc } else { lc }, &mut |ui| {
                        ui.add(egui::Label::new(caps(text)).truncate());
                    });
                }
                ui.end_row();
                for o in &g.rows {
                    cell(ui, champ_w, lc, &mut |ui| {
                        imgs.paint_icon(ui, &o.champ.name, &o.champ.icon, Vec2::splat(22.0), 4.0, Color32::WHITE);
                        let col = if o.is_picked { ACCENT } else { TEXT };
                        ui.add(egui::Label::new(RichText::new(&o.champ.name).size(12.0).color(col)).truncate())
                            .on_hover_text(format!("Ranked #{} of {}", o.rank, g.total));
                    });
                    for (v, heat) in [(o.anyone, false), (o.comfort, false), (o.score, true)] {
                        cell(ui, NUM_COL_W, rc, &mut |ui| {
                            if heat {
                                score_badge(ui, v);
                            } else {
                                ui.label(RichText::new(fmt_signed(v, 1)).size(12.0).color(signed_color(v)));
                            }
                        });
                    }
                    cell(ui, tags_w, lc.with_main_wrap(true), &mut |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(3.0, 2.0);
                        if o.is_picked {
                            chip(ui, "Picked", ACCENT);
                        }
                        if o.is_best_you {
                            chip(ui, "Best for you", POS);
                        }
                        if o.is_best_anyone {
                            chip(ui, "Best for anyone", MUTED);
                        }
                    });
                    ui.end_row();
                }
            });
        if g.total > g.rows.len() {
            ui.label(
                RichText::new(format!("Top of {} ranked candidates, plus your pick.", g.total))
                    .size(10.0)
                    .color(DIM),
            );
        }
    });
}
