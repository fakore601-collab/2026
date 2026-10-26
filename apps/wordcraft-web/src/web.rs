//! The browser shell: web `Services`, drag-and-drop, the unsaved-changes guard and the eframe
//! web runner.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wordcraft_engine::Session;
use wordcraft_ui_egui::async_saves::{AsyncSaves, SaveOutcome};
use wordcraft_ui_egui::theme::{Tokens, medium, regular, semibold};
use wordcraft_ui_egui::{Inbox, Services, UiState, WordApp};

const DOC_EXTS: &[&str] = &["docx", "docm", "dotx", "dotm", "doc", "dot", "odt", "rtf", "txt", "md", "html", "htm", "tex", "json", "xml"];
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "wmf", "emf"];
const RECIPIENT_EXTS: &[&str] = wordcraft_engine::cmd::mailings::RECIPIENT_EXTENSIONS;
mod capacitor;
mod font_store;
const CANVAS_ID: &str = "wordcraft_canvas";
const LOADING_ID: &str = "wordcraft_loading";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AppScreen {
    Documents,
    Editor,
}

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let android = capacitor::is_android();
        let device_languages = capacitor::device_languages();
        let device_lang = wordcraft_ui_egui::i18n::startup_lang(None, device_languages.iter().map(String::as_str));
        let saved_settings = match capacitor::load_settings(android).await {
            Ok(settings) => settings,
            Err(e) if android => {
                log::error!("couldn't load settings: {e}");
                if let Some(el) = document.get_element_by_id(LOADING_ID) {
                    let message =
                        wordcraft_ui_egui::i18n::fmt(wordcraft_ui_egui::i18n::tr(device_lang, "Couldn't load settings: {error}"), &[("error", &e)]);
                    el.set_text_content(Some(&message));
                }
                return;
            }
            Err(e) => {
                log::error!("couldn't load settings: {e}");
                None
            }
        };
        let parsed_settings = saved_settings.as_ref().and_then(|settings| UiState::from_json(settings.as_bytes()));
        if saved_settings.is_some() && parsed_settings.is_none() {
            log::warn!("saved settings were invalid; using defaults");
        }
        let saved_language = parsed_settings.as_ref().map(|settings| settings.language.as_str());
        let startup_lang = wordcraft_ui_egui::i18n::startup_lang(saved_language, device_languages.iter().map(String::as_str));
        let mut ui_settings = parsed_settings.unwrap_or_default();
        if android {
            ui_settings.language = startup_lang.code().to_string();
        }
        if let Some(root) = document.document_element() {
            let _ = root.set_attribute("lang", startup_lang.code());
            let _ = root.set_attribute("dir", if startup_lang.is_rtl() { "rtl" } else { "ltr" });
        }

        let region = wordcraft_ui_egui::i18n::system_region();
        let sample = query().contains("sample");

        // Load Arabic fonts
        if let Err(e) = load_arabic_fonts().await {
            log::error!("Arabic fallback fonts could not be loaded: {e}");
            if let Some(el) = document.get_element_by_id(LOADING_ID) {
                el.set_text_content(Some(&format!("Arabic fallback fonts could not be loaded: {e}")));
            }
            return;
        }

        // Automatic Recovery Check on Android:
        // If clean_exit is false and an active document exists (or recovery.docx exists), restore automatically.
        let mut auto_recovered = None;
        let session_state = if android {
            capacitor::load_session_state().await
        } else {
            capacitor::SessionState::default()
        };

        if android && !session_state.clean_exit {
            let doc_name = session_state.current_doc.clone().unwrap_or_else(|| "recovery.docx".to_string());
            let doc_path = if doc_name.starts_with("Ard/") {
                doc_name.clone()
            } else {
                format!("{}/{}", capacitor::DOCUMENTS_DIR, doc_name)
            };
            if let Ok((bytes, _)) = capacitor::read_private_with_bak(&doc_path).await {
                if let Ok(doc) = wordcraft_engine::io::open_bytes(&doc_name, &bytes) {
                    log::info!("Automatically recovered document: {doc_name}");
                    auto_recovered = Some((doc_name, doc));
                }
            }
            if auto_recovered.is_none() {
                if let Ok((bytes, _)) = capacitor::read_private_with_bak(capacitor::recovery_path()).await {
                    if let Ok(doc) = wordcraft_engine::io::open_bytes("recovery.docx", &bytes) {
                        log::info!("Automatically recovered document from recovery.docx");
                        auto_recovered = Some(("مستند_مسترجع.docx".to_string(), doc));
                    }
                }
            }
        }

        let dirty = Rc::new(Cell::new(false));
        let saves = AsyncSaves::default();
        let saved_file = Rc::new(RefCell::new(None));
        let recovery_cleanup_pending = Rc::new(Cell::new(false));
        if let Err(e) = guard_unload(dirty.clone(), saves.clone()) {
            log::error!("no unsaved-changes guard: {e}");
        }

        let immediate_save_requested = Rc::new(Cell::new(false));
        let back_button_requested = Rc::new(Cell::new(false));

        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }

        let imm_save_vis = immediate_save_requested.clone();
        let imm_save_win = immediate_save_requested.clone();
        let back_btn_cap = back_button_requested.clone();
        let imm_save_cap = immediate_save_requested.clone();

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    if let Some(rs) = &cc.wgpu_render_state {
                        log::info!("wordcraft-web: wgpu backend {:?}", rs.adapter.get_info().backend);
                    }
                    font_store::load_all(cc.egui_ctx.clone());
                    let inbox: Inbox = Inbox::default();
                    let touch_device = web_sys::window()
                        .and_then(|window| window.match_media("(pointer: coarse)").ok().flatten())
                        .is_some_and(|query| query.matches());
                    wordcraft_ui_egui::layout::set_touch_device(&cc.egui_ctx, touch_device);
                    let pasted: Inbox = Inbox::default();
                    if let Err(e) = listen_for_pasted_pictures(pasted.clone(), cc.egui_ctx.clone()) {
                        log::error!("pasting pictures is unavailable: {e}");
                    }

                    // Setup DOM visibilitychange & pause listeners
                    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                        let imm = imm_save_vis.clone();
                        let ctx = cc.egui_ctx.clone();
                        let on_vis = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                            if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                                if d.hidden() {
                                    imm.set(true);
                                    ctx.request_repaint();
                                }
                            }
                        });
                        let _ = doc.add_event_listener_with_callback("visibilitychange", on_vis.as_ref().unchecked_ref());
                        on_vis.forget();
                    }

                    if let Some(win) = web_sys::window() {
                        for ev in ["pause", "pagehide", "beforeunload"] {
                            let imm = imm_save_win.clone();
                            let ctx = cc.egui_ctx.clone();
                            let closure = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                                imm.set(true);
                                ctx.request_repaint();
                            });
                            let _ = win.add_event_listener_with_callback(ev, closure.as_ref().unchecked_ref());
                            closure.forget();
                        }
                    }

                    // Setup Capacitor listeners
                    if android {
                        let btn = back_btn_cap.clone();
                        let ctx = cc.egui_ctx.clone();
                        let _ = capacitor::app_add_listener("backButton", move |_| {
                            btn.set(true);
                            ctx.request_repaint();
                        });

                        let imm = imm_save_cap.clone();
                        let ctx2 = cc.egui_ctx.clone();
                        let _ = capacitor::app_add_listener("pause", move |_| {
                            imm.set(true);
                            ctx2.request_repaint();
                        });
                    }

                    let (initial_doc, initial_name, initial_screen, was_recovered) = if let Some((name, doc)) = auto_recovered {
                        (doc, name, AppScreen::Editor, true)
                    } else if sample {
                        (wordcraft_engine::sample::sample_document(), "sample.docx".to_string(), AppScreen::Editor, false)
                    } else if android {
                        (wordcraft_engine::blank_document(&region), "مستند جديد.docx".to_string(), AppScreen::Documents, false)
                    } else {
                        (wordcraft_engine::blank_document(&region), "مستند جديد.docx".to_string(), AppScreen::Editor, false)
                    };

                    let mut session = Session::new(initial_doc);
                    session.system_region = region;
                    let messages = Rc::new(RefCell::new(Vec::new()));

                    if was_recovered {
                        messages.borrow_mut().push("تم استرجاع عملك".to_string());
                    }

                    let mut app = WordApp::new(
                        session,
                        services(
                            inbox.clone(),
                            cc.egui_ctx.clone(),
                            dirty,
                            saves.clone(),
                            messages.clone(),
                            saved_file.clone(),
                            recovery_cleanup_pending.clone(),
                            android,
                        ),
                    );
                    app.apply_prefs(ui_settings);
                    app.services.pasted = Some(pasted);
                    app.autosave = android;
                    app.session.autosave = android;

                    if !sample && !android && !was_recovered {
                        app.start_screen();
                    }

                    let settings_snapshot = match serde_json::to_string(&app.prefs()) {
                        Ok(settings) => settings,
                        Err(e) => {
                            log::error!("couldn't serialize initial settings: {e}");
                            String::new()
                        }
                    };

                    let docs_inbox = Rc::new(RefCell::new(None));
                    if android {
                        let inbox_init = docs_inbox.clone();
                        let ctx_init = cc.egui_ctx.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            let docs = capacitor::list_documents().await.unwrap_or_default();
                            *inbox_init.borrow_mut() = Some(docs);
                            ctx_init.request_repaint();
                        });
                    }

                    Ok(Box::new(WebShell {
                        app,
                        inbox,
                        messages,
                        saved_file: saved_file.clone(),
                        android,
                        settings_snapshot: Rc::new(RefCell::new(settings_snapshot)),
                        settings_retry_after: Rc::new(Cell::new(0.0)),
                        pending_settings: Rc::new(RefCell::new(None)),
                        settings_write_running: Rc::new(Cell::new(false)),
                        autosave_last: js_sys::Date::now(),
                        last_edit_time: js_sys::Date::now(),
                        last_rev: 0,
                        autosave_running: Rc::new(Cell::new(false)),
                        recovery_cleanup_pending: recovery_cleanup_pending.clone(),
                        screen: initial_screen,
                        current_doc_name: initial_name,
                        documents_list: Vec::new(),
                        documents_loading: android,
                        docs_inbox,
                        confirm_delete: None,
                        rename_dialog: None,
                        immediate_save_requested,
                        back_button_requested,
                    }))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>WordCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}

async fn load_arabic_fonts() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    for name in ["NotoNaskhArabic.ttf", "Amiri-Regular.ttf", "Cairo.ttf"] {
        let url = format!("./fonts/{name}");
        let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&url))
            .await
            .map_err(|e| format!("request for {url} failed: {e:?}"))?
            .dyn_into::<web_sys::Response>()
            .map_err(|e| format!("invalid response for {url}: {e:?}"))?;
        if !response.ok() {
            return Err(format!("request for {url} returned HTTP {}", response.status()));
        }
        let data = wasm_bindgen_futures::JsFuture::from(response.array_buffer().map_err(|e| format!("couldn't read {url}: {e:?}"))?)
            .await
            .map_err(|e| format!("couldn't read {url}: {e:?}"))?;
        let bytes = js_sys::Uint8Array::new(&data).to_vec();
        wordcraft_fonts::FontDb::global().install_font(name, &bytes).map_err(|e| format!("{url}: {e}"))?;
    }
    Ok(())
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

fn format_timestamp(mtime: f64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(mtime));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes()
    )
}

/// Wraps the app to read dropped files asynchronously and feed them through the inbox.
struct WebShell {
    app: WordApp,
    inbox: Inbox,
    messages: Rc<RefCell<Vec<String>>>,
    saved_file: Rc<RefCell<Option<String>>>,
    android: bool,
    settings_snapshot: Rc<RefCell<String>>,
    settings_retry_after: Rc<Cell<f64>>,
    pending_settings: Rc<RefCell<Option<String>>>,
    settings_write_running: Rc<Cell<bool>>,
    autosave_last: f64,
    last_edit_time: f64,
    last_rev: u64,
    autosave_running: Rc<Cell<bool>>,
    recovery_cleanup_pending: Rc<Cell<bool>>,
    screen: AppScreen,
    current_doc_name: String,
    documents_list: Vec<capacitor::DocumentMeta>,
    documents_loading: bool,
    docs_inbox: Rc<RefCell<Option<Vec<capacitor::DocumentMeta>>>>,
    confirm_delete: Option<String>,
    rename_dialog: Option<(String, String)>,
    immediate_save_requested: Rc<Cell<bool>>,
    back_button_requested: Rc<Cell<bool>>,
}

impl eframe::App for WebShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(docs) = self.docs_inbox.borrow_mut().take() {
            self.documents_list = docs;
            self.documents_loading = false;
        }

        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for f in dropped {
            let inbox = self.inbox.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped".into());
                match f.bytes_async().await {
                    Ok(bytes) => {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("couldn't read dropped file {name}: {e}"),
                }
            });
        }
        let msgs = std::mem::take(&mut *self.messages.borrow_mut());
        for msg in msgs {
            self.app.status(msg);
        }

        self.app.logic(ctx);
        self.persist_settings(ctx);

        // Edit rev tracking
        let rev = self.app.session.rev();
        if rev != self.last_rev {
            self.last_rev = rev;
            self.last_edit_time = js_sys::Date::now();
        }

        // Autosave logic: 3-second debounce or immediate trigger
        let now = js_sys::Date::now();
        let debounce_ready = (now - self.last_edit_time) >= 3000.0;
        let immediate = self.immediate_save_requested.replace(false);

        if self.android && self.screen == AppScreen::Editor && self.app.session.dirty && !self.autosave_running.get() {
            if debounce_ready || immediate {
                self.perform_autosave(ctx);
            }
        }

        // Back button handling
        if self.back_button_requested.replace(false) {
            self.handle_back_button(ctx);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match self.screen {
            AppScreen::Documents => self.show_documents_screen(ui),
            AppScreen::Editor => self.show_editor_screen(ui),
        }
    }
}

impl WebShell {
    fn show_editor_screen(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        egui::TopBottomPanel::top("editor_nav_bar")
            .frame(egui::Frame::NONE.fill(t.accent).inner_margin(egui::Margin::symmetric(14, 8)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button(egui::RichText::new("← قائمة المستندات").color(egui::Color32::WHITE).font(semibold(14.0)))
                        .on_hover_text("حفظ والعودة إلى قائمة المستندات")
                        .clicked()
                    {
                        self.save_and_exit_to_documents(ui.ctx());
                    }
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(&self.current_doc_name)
                            .color(egui::Color32::WHITE)
                            .font(medium(14.0)),
                    );
                    if self.app.session.dirty {
                        ui.label(
                            egui::RichText::new("●")
                                .color(egui::Color32::from_rgb(255, 200, 80))
                                .font(semibold(14.0)),
                        )
                        .on_hover_text("توجد تعديلات غير محفوظة");
                    }
                });
            });

        self.app.ui(ui);
    }

    fn show_documents_screen(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());

        egui::TopBottomPanel::top("documents_top_bar")
            .frame(egui::Frame::NONE.fill(t.accent).inner_margin(egui::Margin::symmetric(24, 14)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("أرض").color(egui::Color32::WHITE).font(semibold(20.0)));
                    ui.label(egui::RichText::new("— المستندات").color(egui::Color32::from_white_alpha(190)).font(regular(15.0)));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new("🔄 تحديث").color(egui::Color32::WHITE)).clicked() {
                            self.reload_documents_list(ui.ctx());
                        }
                        if ui
                            .button(egui::RichText::new("+ مستند جديد").color(egui::Color32::WHITE).font(semibold(14.0)))
                            .clicked()
                        {
                            self.create_new_document(ui.ctx());
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(t.ribbon).inner_margin(egui::Margin::symmetric(24, 20)))
            .show_inside(ui, |ui| {
                if self.documents_loading {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.spinner();
                        ui.label("جارٍ تحميل المستندات...");
                    });
                    return;
                }

                if self.documents_list.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label(egui::RichText::new("📄").size(48.0));
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("لا توجد مستندات بعد").font(semibold(18.0)));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("اضغط على زر '+ مستند جديد' للبدء في كتابة مستندك الأول").font(regular(14.0)));
                        ui.add_space(16.0);
                        if ui.button(egui::RichText::new("+ إنشاء مستند جديد").font(semibold(15.0))).clicked() {
                            self.create_new_document(ui.ctx());
                        }
                    });
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    let mut open_doc = None;
                    let mut share_docx_doc = None;
                    let mut share_pdf_doc = None;
                    let mut delete_doc = None;
                    let mut rename_doc = None;

                    for doc in &self.documents_list {
                        egui::Frame::NONE
                            .fill(egui::Color32::WHITE)
                            .stroke(egui::Stroke::new(1.0, t.border_subtle))
                            .rounding(8.0)
                            .inner_margin(egui::Margin::symmetric(16, 12))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("📄").size(24.0));
                                    ui.vertical(|ui| {
                                        let label_resp = ui.link(egui::RichText::new(&doc.name).font(semibold(15.0)));
                                        if label_resp.clicked() {
                                            open_doc = Some(doc.name.clone());
                                        }
                                        let date_str = format_timestamp(doc.mtime);
                                        let size_kb = (doc.size + 1023) / 1024;
                                        ui.label(
                                            egui::RichText::new(format!("آخر تعديل: {date_str} • {size_kb} ك.ب"))
                                                .font(regular(12.0))
                                                .color(egui::Color32::GRAY),
                                        );
                                    });

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("حذف").clicked() {
                                            delete_doc = Some(doc.name.clone());
                                        }
                                        if ui.button("إعادة تسمية").clicked() {
                                            rename_doc = Some(doc.name.clone());
                                        }
                                        if ui.button("PDF 📤").clicked() {
                                            share_pdf_doc = Some(doc.name.clone());
                                        }
                                        if ui.button("DOCX 📤").clicked() {
                                            share_docx_doc = Some(doc.name.clone());
                                        }
                                        if ui.button("فتح").clicked() {
                                            open_doc = Some(doc.name.clone());
                                        }
                                    });
                                });
                            });
                    }

                    if let Some(name) = open_doc {
                        self.open_document_by_name(&name, ui.ctx());
                    }
                    if let Some(name) = share_docx_doc {
                        self.share_docx_document(&name, ui.ctx());
                    }
                    if let Some(name) = share_pdf_doc {
                        self.share_pdf_document(&name, ui.ctx());
                    }
                    if let Some(name) = rename_doc {
                        self.rename_dialog = Some((name.clone(), name));
                    }
                    if let Some(name) = delete_doc {
                        self.confirm_delete = Some(name);
                    }
                });
            });

        // Delete confirmation dialog
        if let Some(name) = self.confirm_delete.clone() {
            let mut confirmed = false;
            let mut cancelled = false;
            egui::Window::new("تأكيد الحذف")
                .id(egui::Id::new("confirm_delete_window"))
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!("هل أنت متأكد من حذف المستند '{name}'؟"));
                    ui.label(egui::RichText::new("لا يمكن التراجع عن هذا الإجراء.").color(egui::Color32::RED));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button(egui::RichText::new("حذف").color(egui::Color32::RED)).clicked() {
                            confirmed = true;
                        }
                        if ui.button("إلغاء").clicked() {
                            cancelled = true;
                        }
                    });
                });
            if confirmed {
                self.confirm_delete = None;
                let ctx = ui.ctx().clone();
                let messages = self.messages.clone();
                let inbox = self.docs_inbox.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = capacitor::delete_private_document(&name).await {
                        messages.borrow_mut().push(format!("فشل حذف المستند: {e}"));
                    } else {
                        messages.borrow_mut().push("تم حذف المستند بنجاح".to_string());
                    }
                    let docs = capacitor::list_documents().await.unwrap_or_default();
                    *inbox.borrow_mut() = Some(docs);
                    ctx.request_repaint();
                });
            } else if cancelled {
                self.confirm_delete = None;
            }
        }

        // Rename dialog
        if let Some((old_name, mut input_name)) = self.rename_dialog.clone() {
            let mut confirmed = false;
            let mut cancelled = false;
            egui::Window::new("إعادة تسمية المستند")
                .id(egui::Id::new("rename_doc_window"))
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("أدخل الاسم الجديد للمستند:");
                    ui.text_edit_singleline(&mut input_name);
                    self.rename_dialog = Some((old_name.clone(), input_name.clone()));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("تأكيد").clicked() {
                            confirmed = true;
                        }
                        if ui.button("إلغاء").clicked() {
                            cancelled = true;
                        }
                    });
                });
            if confirmed {
                self.rename_dialog = None;
                let mut final_name = input_name.trim().to_string();
                if !final_name.ends_with(".docx") {
                    final_name.push_str(".docx");
                }
                if final_name != old_name {
                    let ctx = ui.ctx().clone();
                    let messages = self.messages.clone();
                    let inbox = self.docs_inbox.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        if let Err(e) = capacitor::rename_private_document(&old_name, &final_name).await {
                            messages.borrow_mut().push(format!("فشلت إعادة التسمية: {e}"));
                        } else {
                            messages.borrow_mut().push("تمت إعادة التسمية بنجاح".to_string());
                        }
                        let docs = capacitor::list_documents().await.unwrap_or_default();
                        *inbox.borrow_mut() = Some(docs);
                        ctx.request_repaint();
                    });
                }
            } else if cancelled {
                self.rename_dialog = None;
            }
        }
    }

    fn reload_documents_list(&mut self, ctx: &egui::Context) {
        if !self.android {
            return;
        }
        self.documents_loading = true;
        let inbox = self.docs_inbox.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let docs = capacitor::list_documents().await.unwrap_or_default();
            *inbox.borrow_mut() = Some(docs);
            ctx.request_repaint();
        });
    }

    fn open_document_by_name(&mut self, name: &str, ctx: &egui::Context) {
        self.current_doc_name = name.to_string();
        self.screen = AppScreen::Editor;
        let path = format!("{}/{}", capacitor::DOCUMENTS_DIR, name);
        let name_str = name.to_string();
        let ctx = ctx.clone();
        let messages = self.messages.clone();
        let inbox = self.inbox.clone();

        if self.android {
            let state = capacitor::SessionState {
                current_doc: Some(name.to_string()),
                clean_exit: false,
                last_active: js_sys::Date::now(),
            };
            wasm_bindgen_futures::spawn_local(async move {
                let _ = capacitor::save_session_state(&state).await;
            });
        }

        wasm_bindgen_futures::spawn_local(async move {
            match capacitor::read_private_with_bak(&path).await {
                Ok((bytes, is_bak)) => {
                    if is_bak {
                        messages.borrow_mut().push("تم استرجاع النسخة الاحتياطية (.bak) بنجاح بسبب تلف الملف الأساسي".to_string());
                    }
                    inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name_str, bytes));
                }
                Err(e) => {
                    messages.borrow_mut().push(format!("فشل فتح المستند: {e}"));
                }
            }
            ctx.request_repaint();
        });
    }

    fn create_new_document(&mut self, ctx: &egui::Context) {
        let count = self.documents_list.len() + 1;
        let name = format!("مستند {count}.docx");
        let region = wordcraft_ui_egui::i18n::system_region();
        let doc = wordcraft_engine::blank_document(&region);
        let bytes = match wordcraft_engine::io::save_bytes_with(&name, &doc, None, &Default::default()) {
            Ok(b) => b,
            Err(e) => {
                log::error!("could not create blank document: {e}");
                return;
            }
        };
        let path = format!("{}/{}", capacitor::DOCUMENTS_DIR, name);
        self.current_doc_name = name.clone();
        self.screen = AppScreen::Editor;
        self.app.session.set_document(doc);
        self.app.session.path = None;
        self.app.session.dirty = false;

        if self.android {
            let state = capacitor::SessionState {
                current_doc: Some(name.clone()),
                clean_exit: false,
                last_active: js_sys::Date::now(),
            };
            wasm_bindgen_futures::spawn_local(async move {
                let _ = capacitor::write_private_atomic(&path, &bytes).await;
                let _ = capacitor::save_session_state(&state).await;
            });
        }
        ctx.request_repaint();
    }

    fn share_docx_document(&self, name: &str, ctx: &egui::Context) {
        let path = format!("{}/{}", capacitor::DOCUMENTS_DIR, name);
        let title = name.to_string();
        let messages = self.messages.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match capacitor::get_private_uri(&path).await {
                Ok(uri) => {
                    if let Err(e) = capacitor::share_file_uri(&uri, &title).await {
                        messages.borrow_mut().push(format!("فشلت مشاركة DOCX: {e}"));
                    }
                }
                Err(e) => {
                    messages.borrow_mut().push(format!("فشل تجهيز ملف DOCX للمشاركة: {e}"));
                }
            }
            ctx.request_repaint();
        });
    }

    fn share_pdf_document(&self, name: &str, ctx: &egui::Context) {
        let path = format!("{}/{}", capacitor::DOCUMENTS_DIR, name);
        let name_str = name.to_string();
        let pdf_title = if name.ends_with(".docx") {
            format!("{}.pdf", &name[..name.len() - 5])
        } else {
            format!("{name}.pdf")
        };
        let messages = self.messages.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match capacitor::read_private_with_bak(&path).await {
                Ok((bytes, _)) => {
                    match wordcraft_engine::io::open_bytes(&name_str, &bytes) {
                        Ok(doc) => {
                            let opts = wordcraft_pdf::PdfOptions::default();
                            match wordcraft_pdf::export(&doc, &opts) {
                                Ok(pdf_bytes) => {
                                    let export_path = format!("Ard/exports/{pdf_title}");
                                    if let Err(e) = capacitor::write_private(&export_path, &pdf_bytes).await {
                                        messages.borrow_mut().push(format!("فشل حفظ ملف PDF: {e}"));
                                        ctx.request_repaint();
                                        return;
                                    }
                                    match capacitor::get_private_uri(&export_path).await {
                                        Ok(uri) => {
                                            if let Err(e) = capacitor::share_file_uri(&uri, &pdf_title).await {
                                                messages.borrow_mut().push(format!("فشلت مشاركة PDF: {e}"));
                                            }
                                        }
                                        Err(e) => {
                                            messages.borrow_mut().push(format!("فشل تجهيز ملف PDF للمشاركة: {e}"));
                                        }
                                    }
                                }
                                Err(e) => {
                                    messages.borrow_mut().push(format!("فشل تصدير مستند PDF: {e}"));
                                }
                            }
                        }
                        Err(e) => {
                            messages.borrow_mut().push(format!("فشل قراءة المستند للتصدير: {e}"));
                        }
                    }
                }
                Err(e) => {
                    messages.borrow_mut().push(format!("فشل فتح المستند للتصدير: {e}"));
                }
            }
            ctx.request_repaint();
        });
    }

    fn save_and_exit_to_documents(&mut self, ctx: &egui::Context) {
        self.perform_autosave(ctx);
        self.screen = AppScreen::Documents;
        if self.android {
            let state = capacitor::SessionState {
                current_doc: None,
                clean_exit: true,
                last_active: js_sys::Date::now(),
            };
            wasm_bindgen_futures::spawn_local(async move {
                let _ = capacitor::save_session_state(&state).await;
            });
        }
        self.reload_documents_list(ctx);
    }

    fn handle_back_button(&mut self, ctx: &egui::Context) {
        // 1. Close any open dialogs / popups first
        if self.confirm_delete.is_some() {
            self.confirm_delete = None;
            ctx.request_repaint();
            return;
        }
        if self.rename_dialog.is_some() {
            self.rename_dialog = None;
            ctx.request_repaint();
            return;
        }
        if self.app.dialog.is_some() {
            self.app.dialog = None;
            ctx.request_repaint();
            return;
        }
        if self.app.ui.backstage {
            self.app.ui.backstage = false;
            ctx.request_repaint();
            return;
        }

        // 2. Navigation
        match self.screen {
            AppScreen::Editor => {
                self.save_and_exit_to_documents(ctx);
            }
            AppScreen::Documents => {
                let _ = capacitor::app_exit();
            }
        }
    }

    fn perform_autosave(&mut self, ctx: &egui::Context) {
        if !self.android || self.autosave_running.get() || !self.app.session.dirty {
            return;
        }
        self.last_edit_time = js_sys::Date::now();
        let doc_name = self.current_doc_name.clone();
        let doc_path = format!("{}/{}", capacitor::DOCUMENTS_DIR, doc_name);
        let recovery_path = capacitor::recovery_path().to_string();

        let password = self.app.session.password.as_ref().map(wordcraft_engine::Password::as_str);
        let options = wordcraft_engine::cmd::file::export_options(&self.app.session, &serde_json::json!({}));
        let bytes = match wordcraft_engine::io::save_bytes_with("doc.docx", &self.app.session.doc, password, &options) {
            Ok(b) if b.len() <= MAX_RECOVERY_BYTES => b,
            _ => return,
        };

        self.app.session.dirty = false;
        self.autosave_running.set(true);
        let running = self.autosave_running.clone();
        let messages = self.messages.clone();
        let ctx = ctx.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let res1 = capacitor::write_private_atomic(&doc_path, &bytes).await;
            let res2 = capacitor::write_private_atomic(&recovery_path, &bytes).await;

            if res1.is_ok() && res2.is_ok() {
                messages.borrow_mut().push("تم الحفظ التلقائي".to_string());
            } else if let Err(e) = res1 {
                log::error!("could not autosave to {doc_path}: {e}");
            }
            running.set(false);
            ctx.request_repaint();
        });
    }

    fn persist_settings(&mut self, ctx: &egui::Context) {
        let settings = match serde_json::to_string(&self.app.prefs()) {
            Ok(settings) => settings,
            Err(e) => {
                log::error!("couldn't serialize settings: {e}");
                return;
            }
        };
        if settings.as_str() == self.settings_snapshot.borrow().as_str() || js_sys::Date::now() < self.settings_retry_after.get() {
            return;
        }
        *self.pending_settings.borrow_mut() = Some(settings);
        if self.settings_write_running.replace(true) {
            return;
        }
        let pending = self.pending_settings.clone();
        let writing = self.settings_write_running.clone();
        let snapshot = self.settings_snapshot.clone();
        let retry_after = self.settings_retry_after.clone();
        let android = self.android;
        let messages = self.messages.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                let value = pending.borrow_mut().take();
                let Some(value) = value else { break };
                match capacitor::save_settings(android, value.clone()).await {
                    Ok(()) => {
                        *snapshot.borrow_mut() = value;
                        retry_after.set(0.0);
                    }
                    Err(e) => {
                        log::error!("couldn't save settings: {e}");
                        messages
                            .borrow_mut()
                            .push(wordcraft_ui_egui::i18n::fmt(wordcraft_ui_egui::i18n::t("Couldn't save settings: {error}"), &[("error", &e)]));
                        retry_after.set(js_sys::Date::now() + 5_000.0);
                        break;
                    }
                }
            }
            writing.set(false);
            ctx.request_repaint();
        });
    }
}

const MAX_RECOVERY_BYTES: usize = 200 << 20;
const MAX_PASTED_BYTES: f64 = (200u32 << 20) as f64;
const MAX_PASTED_PICTURES: u32 = 20;

fn listen_for_pasted_pictures(inbox: Inbox, ctx: egui::Context) -> Result<(), String> {
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let on_paste = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(move |e: web_sys::ClipboardEvent| {
        let Some(data) = e.clipboard_data() else { return };
        if !data.get_data("text").unwrap_or_default().is_empty() {
            return;
        }
        let Some(files) = data.files() else { return };
        for i in 0..files.length().min(MAX_PASTED_PICTURES) {
            let Some(file) = files.get(i) else { continue };
            if !file.type_().starts_with("image/") {
                continue;
            }
            if file.size() > MAX_PASTED_BYTES {
                log::warn!("pasted picture is larger than 200 MB; not inserted");
                continue;
            }
            let inbox = inbox.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                    Ok(buf) => {
                        let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push(("pasted.png".into(), bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("couldn't read the pasted picture: {e:?}"),
                }
            });
        }
    });
    document.add_event_listener_with_callback("paste", on_paste.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    on_paste.forget();
    Ok(())
}

fn request_clipboard_text(inbox: wordcraft_ui_egui::ClipboardTextInbox, ctx: egui::Context, messages: Rc<RefCell<Vec<String>>>) {
    let promise = (|| {
        let window = web_sys::window().ok_or_else(|| "no browser window".to_string())?;
        let navigator = js_sys::Reflect::get(window.as_ref(), &wasm_bindgen::JsValue::from_str("navigator")).map_err(|e| format!("{e:?}"))?;
        let clipboard = js_sys::Reflect::get(&navigator, &wasm_bindgen::JsValue::from_str("clipboard")).map_err(|e| format!("{e:?}"))?;
        let read_text = js_sys::Reflect::get(&clipboard, &wasm_bindgen::JsValue::from_str("readText")).map_err(|e| format!("{e:?}"))?;
        if !read_text.is_function() {
            return Err("the browser does not provide clipboard read access".to_string());
        }
        let read_text = js_sys::Function::from(read_text);
        let promise = read_text.call0(&clipboard).map_err(|e| format!("{e:?}"))?;
        Ok(js_sys::Promise::from(promise))
    })();

    let promise = match promise {
        Ok(promise) => promise,
        Err(error) => {
            clipboard_read_failed(&messages, &ctx, &error);
            return;
        }
    };
    wasm_bindgen_futures::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(value) => {
                let Some(text) = value.as_string() else {
                    clipboard_read_failed(&messages, &ctx, "the browser returned no text");
                    return;
                };
                if text.is_empty() {
                    messages.borrow_mut().push(wordcraft_ui_egui::i18n::t("The clipboard is empty.").to_string());
                } else {
                    inbox.lock().unwrap_or_else(|e| e.into_inner()).push(text);
                }
                ctx.request_repaint();
            }
            Err(error) => clipboard_read_failed(&messages, &ctx, &format!("{error:?}")),
        }
    });
}

fn clipboard_read_failed(messages: &Rc<RefCell<Vec<String>>>, ctx: &egui::Context, error: &str) {
    log::warn!("browser clipboard read failed: {error}");
    messages.borrow_mut().push(format!("Couldn't read the clipboard: {error}"));
    ctx.request_repaint();
}

fn services(
    inbox: Inbox,
    ctx: egui::Context,
    dirty: Rc<Cell<bool>>,
    saves: AsyncSaves,
    messages: Rc<RefCell<Vec<String>>>,
    saved_file: Rc<RefCell<Option<String>>>,
    recovery_cleanup_pending: Rc<Cell<bool>>,
    android: bool,
) -> Services {
    let open_inbox = inbox.clone();
    let recent_inbox = inbox.clone();
    let open_ctx = ctx.clone();
    let recent_ctx = ctx.clone();
    let open_messages = messages.clone();
    let recent_messages = messages.clone();
    let save_ctx = ctx.clone();
    let font_ctx = ctx.clone();
    let font_messages = messages.clone();
    let save_messages = messages.clone();
    let clipboard_paste: wordcraft_ui_egui::ClipboardTextInbox = Default::default();
    let requested_clipboard_paste = clipboard_paste.clone();
    let clipboard_ctx = ctx.clone();
    let clipboard_messages = messages.clone();
    let cleanup_ctx = ctx.clone();
    Services {
        open_async: Some(Box::new(move |purpose: &str| {
            let inbox = open_inbox.clone();
            let ctx = open_ctx.clone();
            let messages = open_messages.clone();
            let cache_as_recent = android && purpose == "document";
            Box::new(move |file: Option<rfd::FileHandle>| {
                let Some(file) = file else { return };
                let inbox = inbox.clone();
                let ctx = ctx.clone();
                let messages = messages.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let name = file.file_name();
                    let bytes = file.read().await;
                    if cache_as_recent {
                        let cache_path = capacitor::recent_cache_path(&name);
                        match capacitor::write_private(&cache_path, &bytes).await {
                            Ok(()) => {
                                let recent_path = format!("capacitor-recent/{cache_path}");
                                inbox.lock().unwrap_or_else(|e| e.into_inner()).push((recent_path, bytes));
                            }
                            Err(e) => {
                                log::error!("couldn't cache recent document {name}: {e}");
                                messages.borrow_mut().push(wordcraft_ui_egui::i18n::fmt(
                                    wordcraft_ui_egui::i18n::t("Couldn't cache the recent document: {error}"),
                                    &[("error", &e)],
                                ));
                                inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                            }
                        }
                    } else {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                    }
                    ctx.request_repaint();
                });
            }) as Box<dyn Fn(Option<rfd::FileHandle>)>
        })),
        open_path: Some(Box::new(move |path: &str| {
            let inbox = recent_inbox.clone();
            let ctx = recent_ctx.clone();
            let messages = recent_messages.clone();
            let path = path.to_string();
            Box::new(move || {
                let inbox = inbox.clone();
                let ctx = ctx.clone();
                let messages = messages.clone();
                let path = path.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let read_result = async {
                        let storage_path = capacitor::recent_storage_path(&path)?;
                        let bytes = capacitor::read_private(storage_path).await?;
                        let name = std::path::Path::new(storage_path).file_name().and_then(|name| name.to_str()).unwrap_or("document.docx");
                        Ok::<_, String>((name.to_string(), bytes))
                    }
                    .await;
                    match read_result {
                        Ok((name, bytes)) => {
                            inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                        }
                        Err(e) => {
                            log::error!("couldn't open recent document: {e}");
                            messages.borrow_mut().push(wordcraft_ui_egui::i18n::fmt(
                                wordcraft_ui_egui::i18n::t("Couldn't open the recent document: {error}"),
                                &[("error", &e)],
                            ));
                        }
                    }
                    ctx.request_repaint();
                });
            }) as Box<dyn Fn()>
        })),
        open_recent: Some(Box::new(move |path: &str| {
            let inbox = recent_inbox.clone();
            let ctx = recent_ctx.clone();
            let messages = recent_messages.clone();
            let path = path.to_string();
            Box::new(move |_purpose: &str| {
                let inbox = inbox.clone();
                let ctx = ctx.clone();
                let messages = messages.clone();
                let path = path.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let read_result = async {
                        let storage_path = capacitor::recent_storage_path(&path)?;
                        let bytes = capacitor::read_private(storage_path).await?;
                        let name = std::path::Path::new(storage_path).file_name().and_then(|name| name.to_str()).unwrap_or("document.docx");
                        Ok::<_, String>((name.to_string(), bytes))
                    }
                    .await;
                    match read_result {
                        Ok((name, bytes)) => {
                            inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                        }
                        Err(e) => {
                            log::error!("couldn't open recent document: {e}");
                            messages.borrow_mut().push(wordcraft_ui_egui::i18n::fmt(
                                wordcraft_ui_egui::i18n::t("Couldn't open the recent document: {error}"),
                                &[("error", &e)],
                            ));
                        }
                    }
                    ctx.request_repaint();
                });
            }) as Box<dyn Fn(&str)>
        }),
        pick_save: Some(Box::new(|name: &str| Some(name.to_string()))),
        async_saves: Some(saves.clone()),
        download: Some(Box::new(move |path: &str, bytes: &[u8]| download(&saves, &save_ctx, path, bytes, android, &save_messages, &saved_file))),
        print: Some(Box::new(print_pdf)),
        inbox: Some(inbox),
        clipboard_paste: Some(clipboard_paste),
        request_clipboard_paste: Some(Box::new(move || {
            request_clipboard_text(requested_clipboard_paste.clone(), clipboard_ctx.clone(), clipboard_messages.clone());
        })),
        on_dirty: Some(Box::new(move |d| dirty.set(d))),
        native_storage: android,
        on_saved: android.then(|| {
            Box::new(move || {
                recovery_cleanup_pending.set(true);
                cleanup_ctx.request_repaint();
            }) as Box<dyn Fn()>
        }),
        animations_enabled: Some(animations_enabled()),
        store_font: Some(Box::new(font_store::store)),
        forget_font: Some(Box::new(font_store::forget)),
        load_system_fonts: Some(Box::new(move || {
            load_system_fonts(font_ctx.clone(), font_messages.clone());
        })),
        ..Default::default()
    }
}

const MAX_SYSTEM_FONTS: usize = 500;
const MAX_SYSTEM_FONT_BYTES: usize = 256 << 20;
const MAX_SYSTEM_FONTS_TRIED: u32 = 4000;

fn load_system_fonts(ctx: egui::Context, messages: Rc<RefCell<Vec<String>>>) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let query_fn = match js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("queryLocalFonts")) {
            Ok(val) if val.is_function() => js_sys::Function::from(val),
            _ => {
                messages.borrow_mut().push(wordcraft_ui_egui::i18n::t("Local font access is not supported by your browser.").to_string());
                ctx.request_repaint();
                return;
            }
        };

        let promise = match query_fn.call0(&window) {
            Ok(val) => js_sys::Promise::from(val),
            Err(e) => {
                log::warn!("queryLocalFonts failed: {e:?}");
                messages.borrow_mut().push(wordcraft_ui_egui::i18n::t("Could not query local fonts.").to_string());
                ctx.request_repaint();
                return;
            }
        };

        let fonts_val = match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(val) => val,
            Err(e) => {
                log::warn!("queryLocalFonts rejected: {e:?}");
                return;
            }
        };

        let fonts_arr = match fonts_val.dyn_into::<js_sys::Array>() {
            Ok(arr) => arr,
            Err(_) => return,
        };

        let mut installed = 0;
        let mut total_bytes = 0;
        let len = fonts_arr.length().min(MAX_SYSTEM_FONTS_TRIED);

        for i in 0..len {
            if installed >= MAX_SYSTEM_FONTS || total_bytes >= MAX_SYSTEM_FONT_BYTES {
                break;
            }
            let font_data_obj = fonts_arr.get(i);
            let blob_fn = match js_sys::Reflect::get(&font_data_obj, &wasm_bindgen::JsValue::from_str("blob")) {
                Ok(val) if val.is_function() => js_sys::Function::from(val),
                _ => continue,
            };
            let blob_promise = match blob_fn.call0(&font_data_obj) {
                Ok(val) => js_sys::Promise::from(val),
                _ => continue,
            };
            let blob = match wasm_bindgen_futures::JsFuture::from(blob_promise).await {
                Ok(val) => val,
                _ => continue,
            };
            let array_buffer_fn = match js_sys::Reflect::get(&blob, &wasm_bindgen::JsValue::from_str("arrayBuffer")) {
                Ok(val) if val.is_function() => js_sys::Function::from(val),
                _ => continue,
            };
            let buf_promise = match array_buffer_fn.call0(&blob) {
                Ok(val) => js_sys::Promise::from(val),
                _ => continue,
            };
            let buf = match wasm_bindgen_futures::JsFuture::from(buf_promise).await {
                Ok(val) => val,
                _ => continue,
            };
            let bytes = js_sys::Uint8Array::new(&buf).to_vec();
            if bytes.len() > 20 << 20 {
                continue;
            }

            let postscript_name = js_sys::Reflect::get(&font_data_obj, &wasm_bindgen::JsValue::from_str("postscriptName"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| format!("font_{i}"));

            if wordcraft_fonts::FontDb::global().install_font(&postscript_name, &bytes).is_ok() {
                installed += 1;
                total_bytes += bytes.len();
            }
        }

        messages.borrow_mut().push(wordcraft_ui_egui::i18n::fmt(
            wordcraft_ui_egui::i18n::t("Installed {count} local fonts."),
            &[("count", &installed.to_string())],
        ));
        ctx.request_repaint();
    });
}

fn animations_enabled() -> bool {
    let Some(window) = web_sys::window() else { return true };
    let Ok(media) = window.match_media("(prefers-reduced-motion: reduce)") else { return true };
    media.map(|m| !m.matches()).unwrap_or(true)
}

fn guard_unload(dirty: Rc<Cell<bool>>, saves: AsyncSaves) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let on_unload = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |e: web_sys::BeforeUnloadEvent| {
        if dirty.get() || saves.pending() > 0 {
            e.prevent_default();
            e.set_return_value("unsaved changes");
        }
    });
    window.add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    on_unload.forget();
    Ok(())
}

fn download(
    saves: &AsyncSaves,
    ctx: &egui::Context,
    path: &str,
    bytes: &[u8],
    android: bool,
    messages: &Rc<RefCell<Vec<String>>>,
    saved_file: &Rc<RefCell<Option<String>>>,
) -> Result<(), String> {
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "document".into());
    if android {
        let ticket = saves.start();
        let path = path.to_string();
        let bytes = bytes.to_vec();
        let saves = saves.clone();
        let ctx = ctx.clone();
        let messages = messages.clone();
        let saved_file = saved_file.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match capacitor::save_documents_file(&path, &bytes).await {
                Ok(()) => {
                    *saved_file.borrow_mut() = Some(path.clone());
                    messages.borrow_mut().push(wordcraft_ui_egui::i18n::fmt(wordcraft_ui_egui::i18n::t("Saved to {path}"), &[("path", &path)]));
                    SaveOutcome::Saved
                }
                Err(e) => {
                    log::error!("couldn't save to {path}: {e}");
                    SaveOutcome::Failed(e)
                }
            };
            saves.finish(ticket, outcome);
            ctx.request_repaint();
        });
        return Ok(());
    }
    let window = web_sys::window().ok_or("no window")?;

    if let Ok(picker_val) = js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("showSaveFilePicker"))
        && let Ok(picker_fn) = picker_val.dyn_into::<js_sys::Function>()
    {
        match trigger_save_file_picker(saves, ctx, &window, &picker_fn, &name, bytes) {
            Ok(()) => return Ok(()),
            Err(e) => {
                log::warn!("save file picker failed, falling back to download link: {e}");
            }
        }
    }

    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime_for(&name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(|e| format!("{e:?}"))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|e| format!("{e:?}"))?;

    let a: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| "not an HTML anchor".to_string())?;
    a.set_href(&url);
    a.set_download(&name);
    a.click();
    web_sys::Url::revoke_object_url(&url).ok();
    Ok(())
}

fn trigger_save_file_picker(
    saves: &AsyncSaves,
    ctx: &egui::Context,
    window: &web_sys::Window,
    picker_fn: &js_sys::Function,
    name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &wasm_bindgen::JsValue::from_str("suggestedName"), &wasm_bindgen::JsValue::from_str(name));
    let promise = picker_fn.call1(window, &options).map_err(|e| format!("{e:?}"))?;
    let promise = js_sys::Promise::from(promise);
    let bytes = bytes.to_vec();
    let name = name.to_string();
    let saves = saves.clone();
    let ctx = ctx.clone();
    let ticket = saves.start();
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(handle) => match write_to_file_handle(&handle, &bytes).await {
                Ok(()) => {
                    log::info!("saved {name} through the save picker");
                    SaveOutcome::Saved
                }
                Err(e) => {
                    log::error!("couldn't write {name} to picked file: {e}");
                    SaveOutcome::Failed(e)
                }
            },
            Err(e) => {
                let err_str = format!("{e:?}");
                if err_str.contains("AbortError") {
                    log::debug!("save picker cancelled");
                    SaveOutcome::Cancelled
                } else {
                    log::error!("save picker error: {err_str}");
                    SaveOutcome::Failed(err_str)
                }
            }
        };
        saves.finish(ticket, outcome);
        ctx.request_repaint();
    });
    Ok(())
}

async fn write_to_file_handle(handle: &wasm_bindgen::JsValue, bytes: &[u8]) -> Result<(), String> {
    let create_writable = js_sys::Reflect::get(handle, &wasm_bindgen::JsValue::from_str("createWritable")).map_err(|e| format!("{e:?}"))?;
    let create_writable = js_sys::Function::from(create_writable);
    let stream_promise = js_sys::Promise::from(create_writable.call0(handle).map_err(|e| format!("{e:?}"))?);
    let stream = wasm_bindgen_futures::JsFuture::from(stream_promise).await.map_err(|e| format!("{e:?}"))?;

    let write_fn = js_sys::Reflect::get(&stream, &wasm_bindgen::JsValue::from_str("write")).map_err(|e| format!("{e:?}"))?;
    let write_fn = js_sys::Function::from(write_fn);
    let uint8 = js_sys::Uint8Array::from(bytes);
    let write_promise = js_sys::Promise::from(write_fn.call1(&stream, &uint8).map_err(|e| format!("{e:?}"))?);
    wasm_bindgen_futures::JsFuture::from(write_promise).await.map_err(|e| format!("{e:?}"))?;

    let close_fn = js_sys::Reflect::get(&stream, &wasm_bindgen::JsValue::from_str("close")).map_err(|e| format!("{e:?}"))?;
    let close_fn = js_sys::Function::from(close_fn);
    let close_promise = js_sys::Promise::from(close_fn.call0(&stream).map_err(|e| format!("{e:?}"))?);
    wasm_bindgen_futures::JsFuture::from(close_promise).await.map_err(|e| format!("{e:?}"))?;

    Ok(())
}

const PRINT_FRAME_ID: &str = "wordcraft_print_frame";

fn print_pdf(bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    if let Some(old) = document.get_element_by_id(PRINT_FRAME_ID) {
        if let Ok(frame) = old.clone().dyn_into::<web_sys::HtmlIFrameElement>() {
            let src = frame.src();
            if src.starts_with("blob:") {
                web_sys::Url::revoke_object_url(&src).ok();
            }
        }
        old.remove();
    }

    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;

    let iframe: web_sys::HtmlIFrameElement =
        match document.create_element("iframe").map_err(js).and_then(|e| e.dyn_into().map_err(|_| "not an iframe".to_string())) {
            Ok(f) => f,
            Err(e) => {
                web_sys::Url::revoke_object_url(&url).ok();
                return Err(e);
            }
        };
    iframe.set_id(PRINT_FRAME_ID);
    let attached = iframe
        .style()
        .set_property("display", "none")
        .map_err(js)
        .and_then(|()| document.body().ok_or_else(|| "no body".to_string()))
        .and_then(|body| body.append_child(&iframe).map_err(js));
    if let Err(e) = attached {
        iframe.remove();
        web_sys::Url::revoke_object_url(&url).ok();
        return Err(e);
    }

    let iframe_for_load = iframe.clone();
    let on_load = wasm_bindgen::closure::Closure::once_into_js(move || {
        if let Some(w) = iframe_for_load.content_window() {
            let _ = w.print();
        }
    });
    iframe.set_onload(Some(on_load.unchecked_ref()));
    iframe.set_src(&url);
    Ok(())
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("odt") => "application/vnd.oasis.opendocument.text",
        Some("rtf") => "application/rtf",
        Some("html") => "text/html",
        Some("md" | "txt") => "text/plain",
        Some("tex") => "application/x-tex",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
}
