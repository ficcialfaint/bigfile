#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bigfile::{BigFile, DataSource, error::BigFileError};
use eframe::egui::{
    self, Align, Button, Context, IconData, Id, ImageSource, InnerResponse, Key, KeyboardShortcut,
    Layout, Modal, ModalResponse, Modifiers, TextWrapMode, Ui, Widget,
};
use rfd::FileDialog;
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

// `egui::Context::format_shortcut` displays ⌘ as Cmd,
// which I don't like, so I decided to make my own function.
// Yes, that's the only reason why I ditched `format_shortcut`.
struct Shortcut {
    shortcut: KeyboardShortcut,
    text: &'static str,
}

impl Shortcut {
    const fn new(modifiers: Modifiers, key: Key, macos: &'static str, other: &'static str) -> Self {
        let shortcut = KeyboardShortcut::new(modifiers, key);
        let text = if cfg!(target_os = "macos") {
            macos
        } else {
            other
        };

        Shortcut { shortcut, text }
    }
}

const OPEN_SHORTCUT: Shortcut = Shortcut::new(Modifiers::COMMAND, Key::O, "⌘ O", "Ctrl + O");
const CLOSE_SHORTCUT: Shortcut = Shortcut::new(Modifiers::COMMAND, Key::W, "⌘ W", "Ctrl + W");
const EXTRACT_ALL_SHORTCUT: Shortcut = Shortcut::new(
    Modifiers::COMMAND.plus(Modifiers::SHIFT),
    Key::E,
    "⌘ Shift E",
    "Ctrl + Shift + E",
);
const EXTRACT_SELECTED_SHORTCUT: Shortcut =
    Shortcut::new(Modifiers::COMMAND, Key::E, "⌘ E", "Ctrl + E");

#[derive(Default)]
struct File {
    name: String,
    id: u64,
    path: PathBuf,
}

impl PartialEq for File {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl File {
    fn new(name: String, id: u64, path: PathBuf) -> Self {
        Self { name, id, path }
    }
}

#[derive(Default)]
struct Dir {
    files: Vec<Rc<File>>,
    dirs: BTreeMap<String, Dir>,
}

impl Dir {
    fn from_paths(paths: &Vec<&PathBuf>) -> Dir {
        let mut root = Dir::default();
        let mut id = 0;

        for path in paths {
            root.insert(&path, &mut id, Path::new(""));
        }

        root
    }

    fn insert(&mut self, path: &Path, id: &mut u64, prefix: &Path) {
        let parts: Vec<String> = path
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        if let Some((first, rest)) = parts.split_first() {
            let prefix = prefix.join(first);

            if rest.is_empty() {
                self.files
                    .push(Rc::new(File::new(first.clone(), *id, prefix)));
                *id += 1;
            } else {
                self.dirs.entry(first.clone()).or_default().insert(
                    Path::new(&rest.join("/")),
                    id,
                    &prefix,
                );
            }
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, selected: &mut Vec<Rc<File>>, root: bool) {
        for (dir, subdir) in &mut self.dirs {
            if root {
                subdir.show(ui, selected, false);
            } else {
                egui::CollapsingHeader::new(dir).show(ui, |ui| subdir.show(ui, selected, false));
            }
        }

        self.files.sort_by(|a, b| a.name.cmp(&b.name));

        for file in &self.files {
            let selectable = Button::selectable(selected.contains(&file), &file.name)
                .wrap_mode(TextWrapMode::Extend)
                .ui(ui);

            if selectable.clicked() {
                if ui.input(|i| i.modifiers).command_only() {
                    selected.push(Rc::clone(&file));
                } else {
                    selected.clear();
                    selected.push(Rc::clone(&file));
                }
            }
        }
    }
}

fn open_bigfile(path: &PathBuf) -> bigfile::Result<fs::File> {
    fs::File::open(path).map_err(|err| BigFileError::Io {
        file: Some(path.clone()),
        offset: None,
        err,
    })
}

fn read_bigfile(path: &PathBuf, buf: &mut Vec<u8>) -> bigfile::Result<usize> {
    let mut file = open_bigfile(path)?;
    file.read_to_end(buf).map_err(|err| BigFileError::Io {
        file: Some(path.clone()),
        offset: None,
        err,
    })
}

#[derive(Default)]
struct App {
    bigfile: Option<BigFile>,
    tree: Dir,
    selected: Vec<Rc<File>>,
    bfn_path: Option<PathBuf>,
    bfdb_path: Option<PathBuf>,
    bfdata_path: Option<PathBuf>,
    bigfile_modal: Option<String>,
    error_modal: Option<String>,
    extract_modal: Option<String>,
    preview_image: Option<(PathBuf, Arc<[u8]>)>,
}

impl App {
    fn error(&mut self, text: String) {
        eprintln!("err: {text}");
        self.error_modal = Some(text);
    }

    fn load_bigfile(
        &mut self,
        bfn_path: PathBuf,
        bfdb_path: PathBuf,
        bfdata_path: PathBuf,
    ) -> bigfile::error::Result<()> {
        let bigfile = BigFile::from_paths(bfn_path, bfdb_path, DataSource::File(bfdata_path))?;

        self.tree = Dir::from_paths(&bigfile.entries().keys().collect());
        self.bigfile = Some(bigfile);

        Ok(())
    }

    fn load_bigfile_buf(
        &mut self,
        bfn_path: PathBuf,
        bfdb_path: PathBuf,
        bfdata_path: PathBuf,
    ) -> bigfile::error::Result<()> {
        let mut buf = vec![];
        read_bigfile(&bfdata_path, &mut buf)?;

        let cur = Cursor::new(buf);
        let bfdata = DataSource::Buffer(cur);
        let bigfile = BigFile::from_paths(bfn_path, bfdb_path, bfdata)?;

        self.tree = Dir::from_paths(&bigfile.entries().keys().collect());
        self.bigfile = Some(bigfile);

        Ok(())
    }

    fn show_tree(&mut self, ui: &mut egui::Ui) {
        self.tree.show(ui, &mut self.selected, true);
    }

    fn unload_bigfile(&mut self) {
        self.bigfile = None;
        self.tree = Dir::default();
        self.selected.clear();
        self.bfn_path = None;
        self.bfdb_path = None;
        self.bfdata_path = None;
        self.preview_image = None;
    }

    fn add_bigfile(&mut self) {
        if let Some(bfn_path) = open_bigfile_dialog("bfn")
            && let Some(bfdb_path) = open_bigfile_dialog("bfdb")
            && let Some(bfdata_path) = open_bigfile_dialog("bfdata")
        {
            let text = if let Ok(metadata) = fs::metadata(&bfdata_path) {
                let mb = metadata.len() / 1024 / 1024;
                format!(
                    "{} is {mb} MB in size.\n\
                    Do you want to load the entire file into memory?\n\
                    Pressing \"No\" will read data from disk as needed.",
                    &bfdata_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                )
            } else {
                format!(
                    "Do you want to load the entire file into memory?
                    Pressing \"No\" will read data from disk as needed."
                )
            };

            self.bfn_path = Some(bfn_path);
            self.bfdb_path = Some(bfdb_path);
            self.bfdata_path = Some(bfdata_path);

            self.bigfile_modal = Some(text);
        }
    }

    fn extract_all(&mut self) {
        if let Some(path) = open_extract_dialog()
            && let Some(bigfile) = &self.bigfile
        {
            if let Err(e) = bigfile.extract(path) {
                self.error(format!("{e:?}"));
            } else {
                self.extract_modal = Some(format!(
                    "Finished extracting {} files",
                    bigfile.entries().len()
                ));
            }
        }
    }

    fn common_prefix(&self) -> PathBuf {
        let mut iters: Vec<_> = self
            .selected
            .iter()
            .map(|p| p.path.parent().unwrap_or(Path::new("")).components())
            .collect();

        let mut prefix = PathBuf::new();

        'outer: loop {
            let mut next = None;
            for comps in &mut iters {
                match comps.next() {
                    Some(c) => {
                        if let Some(n) = next {
                            if c != n {
                                break 'outer;
                            }
                        } else {
                            next = Some(c);
                        }
                    }
                    None => break 'outer,
                }
            }
            prefix.push(next.unwrap().as_os_str());
        }

        prefix
    }

    fn extract_selected(&mut self) {
        if let Some(export_path) = open_extract_dialog()
            && let Some(bigfile) = &self.bigfile
        {
            let prefix = self.common_prefix();

            for file in &self.selected {
                match bigfile.get(&file.path) {
                    Ok(v) => {
                        let path =
                            export_path.join(file.path.strip_prefix(&prefix).unwrap_or(&file.path));

                        if let Err(e) = fs::create_dir_all(&path.parent().unwrap()) {
                            // trying to replace it with a self.error() call results in
                            // "cannot borrow *self as mutable" and i cba to figure out a way to fix it
                            let text = format!(
                                "Failed to extract file {}. {e:?}",
                                path.canonicalize().unwrap_or(path).display()
                            );

                            eprintln!("err: {text}");
                            self.error_modal = Some(text);

                            continue;
                        }

                        if let Err(e) = fs::write(&path, v) {
                            // trying to replace it with a self.error() call results in
                            // "cannot borrow *self as mutable" and i cba to figure out a way to fix it
                            let text = format!(
                                "Failed to extract file {}. {e:?}",
                                path.canonicalize().unwrap_or(path).display()
                            );

                            eprintln!("err: {text}");
                            self.error_modal = Some(text);
                        }
                    }
                    Err(e) => {
                        // trying to replace it with a self.error() call results in
                        // "cannot borrow *self as mutable" and i cba to figure out a way to fix it
                        let text =
                            format!("Failed to extract file {}. {e:?}", &file.path.display());

                        eprintln!("err: {text}");
                        self.error_modal = Some(text);
                    }
                };
            }
        }
    }

    fn show_extract_modal(&mut self, ctx: &Context, text: &String) -> ModalResponse<()> {
        show_modal(ctx, "extract".into(), text, |ui| {
            if ui.button("OK").clicked() {
                ui.close();
                self.extract_modal = None;
            }
        })
    }

    fn show_bigfile_modal(&mut self, ctx: &Context, text: &String) -> ModalResponse<()> {
        show_modal(ctx, "bf".into(), &text, |ui| {
            ui.horizontal(|ui| {
                let yes = ui.button("Yes");
                let no = ui.button("No");

                if no.clicked() {
                    if let Err(e) = self.load_bigfile(
                        self.bfn_path.clone().unwrap_or_default(),
                        self.bfdb_path.clone().unwrap_or_default(),
                        self.bfdata_path.clone().unwrap_or_default(),
                    ) {
                        self.error(format!("{e:?}"));
                    }
                    ui.close();
                    self.bigfile_modal = None;
                } else if yes.clicked() {
                    if let Err(e) = self.load_bigfile_buf(
                        self.bfn_path.clone().unwrap_or_default(),
                        self.bfdb_path.clone().unwrap_or_default(),
                        self.bfdata_path.clone().unwrap_or_default(),
                    ) {
                        self.error(format!("{e:?}"));
                    }
                    ui.close();
                    self.bigfile_modal = None;
                }
            });
        })
    }

    fn show_error_modal(&mut self, ctx: &Context, err: &String) -> ModalResponse<()> {
        Modal::new(Id::new("err")).show(ctx, |ui| {
            ui.heading("ERROR!");
            ui.label(err);

            ui.add_space(32.0);

            if ui.button("OK").clicked() {
                ui.close();
                self.error_modal = None;
            }
        })
    }

    fn show_modals(&mut self, ctx: &Context) {
        if let Some(text) = self.error_modal.clone() {
            self.show_error_modal(ctx, &text);
        }

        if let Some(text) = self.bigfile_modal.clone() {
            self.show_bigfile_modal(ctx, &text);
        }

        if let Some(text) = self.extract_modal.clone() {
            self.show_extract_modal(ctx, &text);
        }
    }

    fn show_menu(&mut self, ctx: &Context) -> InnerResponse<()> {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    ui.vertical(|ui| {
                        let open = Button::new("Open").shortcut_text(OPEN_SHORTCUT.text);
                        let close = Button::new("Close").shortcut_text(CLOSE_SHORTCUT.text);
                        let extract =
                            Button::new("Extract All").shortcut_text(EXTRACT_ALL_SHORTCUT.text);

                        if open.ui(ui).clicked() {
                            self.add_bigfile();
                        }

                        if ui.add_enabled(self.bigfile.is_some(), close).clicked() {
                            self.unload_bigfile();
                        }

                        if ui.add_enabled(self.bigfile.is_some(), extract).clicked() {
                            self.extract_all();
                        }
                    })
                });

                ui.menu_button("Selection", |ui| {
                    ui.vertical(|ui| {
                        let btn = Button::new("Extract Selected")
                            .shortcut_text(EXTRACT_SELECTED_SHORTCUT.text);
                        if ui.add_enabled(!self.selected.is_empty(), btn).clicked() {
                            self.extract_selected();
                        }
                    })
                });
            });
        })
    }

    fn show_left_panel(&mut self, ctx: &Context) {
        egui::SidePanel::left("left_panel")
            .resizable(true)
            .width_range(80.0..=640.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.show_tree(ui);
                });
            });
    }

    fn show_bottom_panel(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.bigfile.is_some() {
                    ui.label(format!(
                        "{} • {} • {}",
                        self.bfn_path
                            .clone()
                            .unwrap_or_default()
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        self.bfdb_path
                            .clone()
                            .unwrap_or_default()
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        self.bfdata_path
                            .clone()
                            .unwrap_or_default()
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    ));
                }

                ui.with_layout(Layout::right_to_left(Align::RIGHT), |ui| {
                    ui.label(format!("v{APP_VERSION}"))
                });
            });
        });
    }

    fn display_preview(&mut self, ui: &mut Ui) {
        if !self.selected.is_empty()
            && let Some(image) = self.get_current_preview_file(ui)
        {
            ui.centered_and_justified(|ui| {
                ui.image(ImageSource::Bytes {
                    uri: format!("bytes://{}", &self.selected[0].path.to_string_lossy()).into(),
                    bytes: image.into(),
                })
            });
        }
    }
    fn get_current_preview_file(&mut self, ui: &mut Ui) -> Option<Arc<[u8]>> {
        if let Some(img) = &self.preview_image
            && img.0 == self.selected[0].path
        {
            return Some(img.1.clone());
        }

        if let Some(bigfile) = &self.bigfile
            && let Ok(image) = bigfile.get(&self.selected[0].path)
        {
            if let Some(img) = &self.preview_image {
                let key = format!("bytes://{}", img.0.to_string_lossy());
                ui.ctx().forget_image(&key);
            }

            let ptr: Arc<[u8]> = image.into();
            self.preview_image = Some((self.selected[0].path.clone(), ptr.clone()));
            return Some(ptr);
        }
        None
    }

    fn handle_input(&mut self, ctx: &Context) {
        ctx.input_mut(|i| {
            if i.consume_shortcut(&OPEN_SHORTCUT.shortcut) {
                self.add_bigfile();
            }

            if self.bigfile.is_some() && i.consume_shortcut(&CLOSE_SHORTCUT.shortcut) {
                self.unload_bigfile();
            }

            if self.bigfile.is_some() && i.consume_shortcut(&EXTRACT_ALL_SHORTCUT.shortcut) {
                self.extract_all();
            }

            if !self.selected.is_empty() && i.consume_shortcut(&EXTRACT_SELECTED_SHORTCUT.shortcut)
            {
                self.extract_selected();
            }
        })
    }
}

fn open_extract_dialog() -> Option<PathBuf> {
    FileDialog::new()
        .set_title("Select extract directory")
        .pick_folder()
}

fn open_bigfile_dialog(extension: &str) -> Option<PathBuf> {
    FileDialog::new()
        .set_title(format!("Choose bigfile.{extension} file"))
        .add_filter("bigfile", &[extension])
        .pick_file()
}

fn show_modal<T>(
    ctx: &Context,
    id: String,
    text: &String,
    content: impl FnOnce(&mut Ui) -> T,
) -> ModalResponse<T> {
    Modal::new(Id::new(id)).show(ctx, |ui| {
        ui.label(text);
        ui.add_space(32.0);

        content(ui)
    })
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.handle_input(ctx);
        self.show_menu(ctx);
        self.show_bottom_panel(ctx);

        if self.bigfile.is_some() {
            self.show_left_panel(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.bigfile.is_some() {
                self.display_preview(ui);
            }
        });
        self.show_modals(ctx);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_icon(IconData::default()),
        ..Default::default()
    };

    eframe::run_native(
        "bigfile",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::<App>::default())
        }),
    )
}
