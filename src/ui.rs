use gtk::{
    Application, ApplicationWindow, Box as GtkBox, Button, ButtonsType, CheckButton, Entry,
    FileChooserAction, FileChooserNative, FileFilter, HeaderBar, Label, MessageDialog, MessageType,
    Orientation, ResponseType, Spinner, prelude::*,
};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::mpsc, time::Duration};

#[derive(Clone, Copy)]
struct Language {
    italian: bool,
}

impl Language {
    fn system() -> Self {
        let locale = sys_locale::get_locale().unwrap_or_default();
        Self {
            italian: locale
                .split(['-', '_', '.', '@'])
                .next()
                .is_some_and(|language| language.eq_ignore_ascii_case("it")),
        }
    }

    fn text(self, italian: &'static str, english: &'static str) -> &'static str {
        if self.italian { italian } else { english }
    }
}

pub fn run() {
    let app = Application::builder()
        .application_id("com.andrez.fmblueprint")
        .build();
    app.connect_activate(build_window);
    app.run();
}

fn build_window(app: &Application) {
    let language = Language::system();
    let root = GtkBox::new(Orientation::Vertical, 14);
    root.set_margin_top(20);
    root.set_margin_bottom(20);
    root.set_margin_start(22);
    root.set_margin_end(22);

    let heading = Label::new(Some(language.text(
        "Crea la documentazione del progetto FileMaker",
        "Create FileMaker project documentation",
    )));
    heading.add_css_class("title-2");
    heading.set_xalign(0.0);
    root.append(&heading);

    let input_path = Rc::new(RefCell::new(None::<PathBuf>));
    let output_to_open = Rc::new(RefCell::new(None::<PathBuf>));
    let input = Entry::builder()
        .editable(false)
        .hexpand(true)
        .placeholder_text(language.text("Seleziona un file XML", "Select an XML file"))
        .build();
    let input_browse = Button::with_label(language.text("Sfoglia…", "Browse…"));
    let input_row = labeled_row(language.text("File XML", "XML file"), &input, &input_browse);
    root.append(&input_row);

    let output = Entry::builder()
        .hexpand(true)
        .placeholder_text(language.text("Cartella di output", "Output folder"))
        .build();
    let output_browse = Button::with_label(language.text("Scegli…", "Choose…"));
    root.append(&labeled_row(
        language.text("Output", "Output"),
        &output,
        &output_browse,
    ));

    let create_html = CheckButton::with_label(
        language.text("Crea riepilogo HTML interattivo", "Create HTML recap"),
    );
    root.append(&create_html);

    let status_row = GtkBox::new(Orientation::Horizontal, 8);
    let spinner = Spinner::new();
    spinner.set_visible(false);
    let status = Label::new(Some(language.text(
        "Seleziona un file XML per iniziare.",
        "Select an XML file to begin.",
    )));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_hexpand(true);
    status_row.append(&spinner);
    status_row.append(&status);
    root.append(&status_row);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let open_folder = Button::with_label(language.text("Apri cartella", "Open folder"));
    open_folder.set_visible(false);
    output.connect_changed({
        let output_to_open = output_to_open.clone();
        let open_folder = open_folder.clone();
        let status = status.clone();
        move |_| {
            if output_to_open.borrow_mut().take().is_some() {
                open_folder.set_visible(false);
                status.set_text(language.text(
                    "Cartella modificata; avvia una nuova analisi.",
                    "Output folder changed; run analysis again.",
                ));
            }
        }
    });
    let run = Button::with_label(language.text("Analizza", "Run analysis"));
    run.add_css_class("suggested-action");
    actions.append(&open_folder);
    actions.append(&run);
    root.append(&actions);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("fmblueprint")
        .default_width(620)
        .default_height(340)
        .child(&root)
        .build();
    let header = HeaderBar::new();
    header.set_title_widget(Some(&Label::new(Some("fmblueprint"))));
    window.set_titlebar(Some(&header));

    input_browse.connect_clicked({
        let window = window.clone();
        let input = input.clone();
        let output = output.clone();
        let input_path = input_path.clone();
        let output_to_open = output_to_open.clone();
        let open_folder = open_folder.clone();
        let status = status.clone();
        move |_| {
            let dialog = FileChooserNative::new(
                Some(language.text("Seleziona il file XML", "Select XML file")),
                Some(&window),
                FileChooserAction::Open,
                Some(language.text("Apri", "Open")),
                Some(language.text("Annulla", "Cancel")),
            );
            let filter = FileFilter::new();
            filter.set_name(Some("XML"));
            filter.add_pattern("*.xml");
            filter.add_pattern("*.XML");
            dialog.add_filter(&filter);
            dialog.connect_response({
                let input = input.clone();
                let output = output.clone();
                let input_path = input_path.clone();
                let output_to_open = output_to_open.clone();
                let open_folder = open_folder.clone();
                let status = status.clone();
                move |dialog, response| {
                    if response == ResponseType::Accept
                        && let Some(path) = dialog.file().and_then(|file| file.path())
                    {
                        *output_to_open.borrow_mut() = None;
                        open_folder.set_visible(false);
                        input.set_text(&path.to_string_lossy());
                        output.set_text(
                            &crate::default_output_dir(std::slice::from_ref(&path))
                                .display()
                                .to_string(),
                        );
                        *input_path.borrow_mut() = Some(path);
                        status.set_text(language.text(
                            "File selezionato. Pronto per l'analisi.",
                            "File selected. Ready to analyze.",
                        ));
                    }
                    dialog.destroy();
                }
            });
            dialog.show();
        }
    });

    output_browse.connect_clicked({
        let window = window.clone();
        let output = output.clone();
        move |_| {
            let dialog = FileChooserNative::new(
                Some(language.text("Seleziona la cartella di output", "Select output folder")),
                Some(&window),
                FileChooserAction::SelectFolder,
                Some(language.text("Seleziona", "Select")),
                Some(language.text("Annulla", "Cancel")),
            );
            dialog.connect_response({
                let output = output.clone();
                move |dialog, response| {
                    if response == ResponseType::Accept
                        && let Some(path) = dialog.file().and_then(|file| file.path())
                    {
                        output.set_text(&path.to_string_lossy());
                    }
                    dialog.destroy();
                }
            });
            dialog.show();
        }
    });

    open_folder.connect_clicked({
        let output_to_open = output_to_open.clone();
        let status = status.clone();
        move |_| {
            let Some(path) = output_to_open.borrow().clone() else {
                return;
            };
            let uri = gtk::gio::File::for_path(&path).uri();
            if let Err(error) =
                gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>)
            {
                status.set_text(&format!(
                    "{} {error}",
                    language.text("Impossibile aprire la cartella:", "Could not open folder:")
                ));
            }
        }
    });

    let start_analysis: Rc<dyn Fn()> = Rc::new({
        let input_path = input_path.clone();
        let output = output.clone();
        let create_html = create_html.clone();
        let run = run.clone();
        let input_browse = input_browse.clone();
        let output_browse = output_browse.clone();
        let spinner = spinner.clone();
        let status = status.clone();
        let open_folder = open_folder.clone();
        let output_to_open = output_to_open.clone();
        move || {
            let Some(input_path) = input_path.borrow().clone() else {
                status.set_text(language.text("Seleziona un file XML.", "Select an XML file."));
                return;
            };
            let output_text = output.text();
            if output_text.trim().is_empty() {
                status.set_text(
                    language.text("Indica la cartella di output.", "Choose an output folder."),
                );
                return;
            }
            let output_path = PathBuf::from(output_text.as_str());
            let generate_html = create_html.is_active();
            *output_to_open.borrow_mut() = None;
            open_folder.set_visible(false);
            run.set_sensitive(false);
            input_browse.set_sensitive(false);
            output_browse.set_sensitive(false);
            create_html.set_sensitive(false);
            spinner.set_visible(true);
            spinner.start();
            status.set_text(language.text("Analisi in corso…", "Analyzing…"));

            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = crate::run_analysis_with_options(
                    input_path,
                    output_path.clone(),
                    generate_html,
                );
                let _ = sender.send((result, output_path));
            });

            gtk::glib::timeout_add_local(Duration::from_millis(100), {
                let run = run.clone();
                let input_browse = input_browse.clone();
                let output_browse = output_browse.clone();
                let create_html = create_html.clone();
                let spinner = spinner.clone();
                let status = status.clone();
                let open_folder = open_folder.clone();
                let output_to_open = output_to_open.clone();
                move || match receiver.try_recv() {
                    Ok((result, output_path)) => {
                        spinner.stop();
                        spinner.set_visible(false);
                        run.set_sensitive(true);
                        input_browse.set_sensitive(true);
                        output_browse.set_sensitive(true);
                        create_html.set_sensitive(true);
                        match result {
                            Ok(()) => {
                                status.set_text(&format!(
                                    "{} {}",
                                    language
                                        .text("Documentazione creata:", "Documentation created:"),
                                    output_path.display()
                                ));
                                *output_to_open.borrow_mut() = Some(output_path);
                                open_folder.set_visible(true);
                            }
                            Err(error) => {
                                status.set_text(&format!(
                                    "{} {error:#}",
                                    language.text("Analisi non riuscita:", "Analysis failed:")
                                ));
                            }
                        }
                        gtk::glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        spinner.stop();
                        spinner.set_visible(false);
                        run.set_sensitive(true);
                        input_browse.set_sensitive(true);
                        output_browse.set_sensitive(true);
                        create_html.set_sensitive(true);
                        status.set_text(language.text(
                            "Il processo di analisi si è interrotto.",
                            "The analysis worker stopped unexpectedly.",
                        ));
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        }
    });

    run.connect_clicked({
        let start_analysis = start_analysis.clone();
        let input_path = input_path.clone();
        let output = output.clone();
        let status = status.clone();
        let window = window.clone();
        move |_| {
            if input_path.borrow().is_none() {
                status.set_text(language.text("Seleziona un file XML.", "Select an XML file."));
                return;
            }
            let output_path = PathBuf::from(output.text().as_str());
            match crate::output_dir_is_nonempty(&output_path) {
                Ok(false) => start_analysis(),
                Err(error) => status.set_text(&format!("{error:#}")),
                Ok(true) => {
                    let dialog = MessageDialog::builder()
                        .transient_for(&window)
                        .modal(true)
                        .message_type(MessageType::Warning)
                        .buttons(ButtonsType::YesNo)
                        .text(language.text(
                            "La cartella di output contiene già dei file.",
                            "The output folder already contains files.",
                        ))
                        .secondary_text(language.text(
                            "I file esistenti potrebbero essere sovrascritti o rimossi. Continuare?",
                            "Existing files may be overwritten or removed. Continue?",
                        ))
                        .build();
                    dialog.set_default_response(ResponseType::No);
                    dialog.connect_response({
                        let start_analysis = start_analysis.clone();
                        move |dialog, response| {
                            if response == ResponseType::Yes {
                                start_analysis();
                            }
                            dialog.close();
                        }
                    });
                    dialog.present();
                }
            }
        }
    });

    window.present();
}

fn labeled_row(label: &str, entry: &Entry, button: &Button) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    let caption = Label::new(Some(label));
    caption.set_width_chars(10);
    caption.set_xalign(0.0);
    row.append(&caption);
    row.append(entry);
    row.append(button);
    row
}
