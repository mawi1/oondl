use std::borrow::Cow;
use std::path::PathBuf;

use arboard::Clipboard;
use directories::UserDirs;
use eframe::glow::Context;
use egui::{vec2, Align2, Pos2, RichText, Ui, Vec2};
use egui_file::FileDialog;
use permissions::is_writable;
use serde::{Deserialize, Serialize};

use super::downloader::{Client, DownloadRequest, OonUrl, Phase, Quality, State};

#[derive(Deserialize, Serialize)]
struct DownloadForm {
    url: String,
    quality: Quality,
    dest_dir: Option<PathBuf>,
}

impl Default for DownloadForm {
    fn default() -> Self {
        let video_dir = (|| {
            let ud = UserDirs::new()?;
            ud.video_dir()
                .or(ud.download_dir())
                .map(|p| p.to_path_buf())
        })();

        DownloadForm {
            url: "".to_owned(),
            quality: Quality::High,
            dest_dir: video_dir,
        }
    }
}

impl DownloadForm {
    fn reset(&mut self) {
        self.url = "".to_owned();
    }

    fn is_valid(&self) -> bool {
        !self.url.is_empty() && self.dest_dir.is_some()
    }
}

pub struct OondlApp {
    download_form: DownloadForm,
    maybe_clipboard: Option<Clipboard>,
    open_file_dialog: Option<FileDialog>,
    show_invalid_url: bool,
    show_dest_dir_not_writeable: bool,
    client: Client,
    state: State,
}

impl OondlApp {
    pub fn new(cc: &eframe::CreationContext<'_>, client: Client) -> Self {
        let download_form = if let Some(storage) = cc.storage {
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            DownloadForm::default()
        };

        Self {
            download_form,
            maybe_clipboard: Clipboard::new().ok(),
            open_file_dialog: None,
            show_invalid_url: false,
            show_dest_dir_not_writeable: false,
            client,
            state: State::new(),
        }
    }
}

impl eframe::App for OondlApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        const SPACE: f32 = 3.0;
        const SPACE_2: f32 = 6.0;
        const SPACE_4: f32 = 12.0;

        while let Some(u) = self.client.poll_update() {
            self.state.update(u);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_enabled(
                !self.show_invalid_url
                    && !self.show_dest_dir_not_writeable
                    && !self.state.has_error(),
            );
            ui.add_space(3.0);
            egui::Grid::new("pos_size")
                .num_columns(2)
                .spacing([SPACE_4, SPACE_2])
                .show(ui, |ui| {
                    let url_label = ui.label("URL:");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("📋 Einfügen").clicked() {
                            if let Some(cb) = &mut self.maybe_clipboard {
                                if let Ok(text) = cb.get_text() {
                                    self.download_form.url = text;
                                }
                            }
                        }
                        let te = egui::TextEdit::singleline(&mut self.download_form.url)
                            .desired_width(f32::INFINITY)
                            .hint_text("z.B. https://on.orf.at/video/12345678");
                        ui.add_sized(ui.available_size(), te)
                            .labelled_by(url_label.id);
                    });
                    ui.end_row();

                    ui.label("Qualität:");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.download_form.quality, Quality::High, "Hoch");
                        ui.radio_value(&mut self.download_form.quality, Quality::Medium, "Mittel");
                        ui.radio_value(&mut self.download_form.quality, Quality::Low, "Niedrig");
                    });
                    ui.end_row();

                    ui.label("Zielordner:");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("📁 Auswählen").clicked() {
                            let mut dialog =
                                FileDialog::select_folder(self.download_form.dest_dir.clone())
                                    .default_size(vec2(480.0, 350.0))
                                    .resizable(false)
                                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO);
                            dialog.open();
                            self.open_file_dialog = Some(dialog);
                        }

                        fn uneditable_textedit(ui: &mut egui::Ui, mut text: &str) {
                            ui.add_sized(
                                ui.available_size(),
                                egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY),
                            );
                        }
                        let mut path_cow = Cow::Borrowed("");
                        if let Some(p) = &self.download_form.dest_dir {
                            path_cow = p.to_string_lossy();
                        }

                        uneditable_textedit(ui, path_cow.as_ref());
                    });
                    ui.end_row();
                });

            if let Some(dialog) = &mut self.open_file_dialog {
                if dialog.show(ctx).selected() {
                    if let Some(dir_path) = dialog.path() {
                        self.download_form.dest_dir = Some(dir_path.to_path_buf());
                    }
                }
            };

            ui.add_space(SPACE_4);

            ui.add_enabled_ui(self.download_form.is_valid(), |ui| {
                if ui.button("Download").clicked() {
                    let url_res = OonUrl::new(&self.download_form.url.trim());
                    let dest_dir_writeable =
                        is_writable(self.download_form.dest_dir.as_ref().unwrap()).is_ok_and(|w| w);
                    if url_res.is_ok() && dest_dir_writeable {
                        self.client.add_download(
                            DownloadRequest::new(
                                url_res.unwrap(),
                                self.download_form.quality,
                                self.download_form.dest_dir.as_ref().unwrap().clone(),
                            ),
                            &mut self.state,
                        );
                        self.download_form.reset();
                    } else {
                        self.show_invalid_url = url_res.is_err();
                        self.show_dest_dir_not_writeable = !dest_dir_writeable;
                    }
                }
            });

            ui.add_space(SPACE_4);

            if self.state.phase() == Phase::Idle {
                ui.vertical(|ui| {
                    ui.set_height(123_f32);
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Kein aktiver Download.").size(14.0));
                    })
                });
            } else {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.add_space(SPACE_2);
                    let title = self.state.title().unwrap_or("<Titel>");
                    ui.label(RichText::new(title).strong().underline().size(14.0));
                    ui.add_space(SPACE_2);
                    match self.state.phase() {
                        Phase::Analyzing => {
                            ui.label("Analysieren");
                            ui.add_space(SPACE);
                            ui.spinner();
                        }
                        Phase::Downloading { progress, video_no } => {
                            ui.label(format!(
                                "Herunterladen {:.0}% Video {} von {}",
                                progress * 100_f32,
                                video_no.0,
                                video_no.1,
                            ));
                            let pbar = egui::ProgressBar::new(progress);
                            ui.add_space(SPACE);
                            ui.add(pbar);
                        }
                        Phase::Merging => {
                            ui.label("Zusammenfügen");
                            ui.add_space(SPACE);
                            ui.spinner();
                        }
                        Phase::Idle => unreachable!(),
                    }
                    ui.add_space(SPACE_4);
                    if ui.button("Abbrechen").clicked() {
                        self.client.cancel_download();
                    }
                    ui.add_space(SPACE_2);
                });
            }

            ui.add_space(SPACE_4);

            ui.label(
                RichText::new("Warteschlange")
                    .underline()
                    .size(14.0)
                    .strong(),
            );

            if self.state.queue_is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Warteschlange ist leer.").size(14.0));
                });
            } else {
                ui.add_space(SPACE_4);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.state.retain_from_queue(|q| {
                        let mut keep = true;

                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.add_space(SPACE_2);
                            ui.label(&q.title);
                            ui.add_space(SPACE_4);
                            if ui.button("Entfernen").clicked() {
                                self.client.delete_download(q.request_id);
                                keep = false;
                            };
                            ui.add_space(SPACE_2);
                        });
                        ui.add_space(SPACE_2);

                        keep
                    });
                });
            }
        });

        fn error_modal(ctx: &egui::Context, add_contents: impl FnOnce(&mut Ui)) {
            egui::Window::new("Fehler!")
                .collapsible(false)
                .pivot(Align2::CENTER_TOP)
                .fixed_pos(Pos2::new(300.0, 30.0))
                .show(ctx, |ui| {
                    ui.set_width(300.0);
                    ui.add_space(SPACE_4);
                    add_contents(ui);
                    ui.add_space(SPACE);
                });
        }

        if self.show_invalid_url || self.show_dest_dir_not_writeable {
            error_modal(ctx, |ui| {
                if self.show_invalid_url {
                    ui.label(RichText::new("Keine gültige Url.").size(14.0));
                }
                if self.show_dest_dir_not_writeable {
                    ui.label(RichText::new("Keine Schreibrechte für Zielordner.").size(14.0));
                }
                ui.add_space(SPACE_4);
                if ui.button("OK").clicked() {
                    self.show_invalid_url = false;
                    self.show_dest_dir_not_writeable = false;
                }
            });
        }

        if self.state.has_error() {
            error_modal(ctx, |ui| {
                let err_message = match self.state.error().unwrap() {
                    crate::downloader::Error::NetworkError(_) => {
                        "Ein Netzwerkfehler ist aufgetreten."
                    }
                    crate::downloader::Error::FileError(_) => "Fehler beim schreiben einer Datei.",
                    crate::downloader::Error::UnexpectedError(_) => {
                        "Es ist ein unerwarteter Fehler aufgetreten."
                    }
                };
                ui.label(RichText::new(err_message).size(14.0));
                ui.add_space(SPACE_4);
                ui.horizontal(|ui| {
                    if ui.button("Abbrechen").clicked() {
                        self.client.cancel_on_error();
                    }
                    if ui.button("Wiederholen").clicked() {
                        self.client.retry();
                    }
                });
            });
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.download_form);
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn on_exit(&mut self, _gl: Option<&Context>) {
        self.client.shutdown();
        log::debug!("gui exited");
    }
}
