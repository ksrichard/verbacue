//! Settings: speech to text, translation, output, and the models on this computer.

use std::path::PathBuf;

use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, pick_list, row, scrollable, text, text_input},
};
use verbacue::{
    models::{self, Kind},
    subtitle::Format,
    translate::{Backend, Register},
};

use crate::{
    app::{App, Message},
    settings::Settings,
    theme,
    widgets::switch,
};

/// One change on this screen.
#[derive(Debug, Clone)]
pub enum SettingChange {
    WhisperModel(String),
    Language(Option<String>),
    Vad(bool),
    Translator(Backend),
    LlmModel(String),
    Endpoint(String),
    EndpointModel(String),
    Register(Register),
    Glossary(Option<PathBuf>),
    FilmNotes(bool),
    Format(Format),
    OutDir(Option<PathBuf>),
}

impl SettingChange {
    pub fn apply(self, s: &mut Settings) {
        match self {
            Self::WhisperModel(v) => s.whisper_model = v,
            Self::Language(v) => s.language = v,
            Self::Vad(v) => s.vad = v,
            Self::Translator(v) => s.translator = v,
            Self::LlmModel(v) => s.llm_model = v,
            Self::Endpoint(v) => s.endpoint = v,
            Self::EndpointModel(v) => s.endpoint_model = (!v.is_empty()).then_some(v),
            Self::Register(v) => s.register = v,
            Self::Glossary(v) => s.glossary = v,
            Self::FilmNotes(v) => s.film_notes = v,
            Self::Format(v) => s.format = v,
            Self::OutDir(v) => s.out_dir = v,
        }
    }
}

/// The server URL and model fields show only for an OpenAI-compatible server.
pub fn shows_server_fields(settings: &Settings) -> bool {
    settings.translator == Backend::OpenaiCompat
}

/// The environment variable and key file of an online service.
fn key_source(backend: Backend) -> Option<(&'static str, &'static str)> {
    match backend {
        Backend::Deepl => Some(("DEEPL_API_KEY", "deepl.key")),
        Backend::Google => Some(("GOOGLE_TRANSLATE_API_KEY", "google.key")),
        Backend::Azure => Some(("AZURE_TRANSLATOR_KEY", "azure.key")),
        Backend::OpenaiCompat => Some(("OPENAI_API_KEY", "openai.key")),
        Backend::Local | Backend::None => None,
    }
}

fn key_found(var: &str, file: &str) -> bool {
    std::env::var_os(var).is_some_and(|v| !v.is_empty())
        || verbacue::config_dir().is_some_and(|d| d.join(file).is_file())
}

/// A choice in a list, shown with a friendly name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice<T> {
    value: T,
    label: String,
}

impl<T> std::fmt::Display for Choice<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn model_choices(kind: Kind) -> Vec<Choice<String>> {
    models::known()
        .iter()
        .filter(|m| m.kind == kind)
        .map(|m| Choice {
            value: m.name.to_string(),
            label: format!("{} ({})", m.name, models::human_size(m.size)),
        })
        .collect()
}

fn setting_row<'a>(
    title: &'a str,
    hint: &'a str,
    control: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![
        column![
            text(title).font(theme::BODY_BOLD).color(theme::TEXT),
            text(hint).size(13).color(theme::MUTED),
        ]
        .spacing(3)
        .width(Length::Fill),
        control.into(),
    ]
    .spacing(16)
    .align_y(Alignment::Center)
    .into()
}

fn card<'a>(title: &'a str, rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    container(
        column![text(title).font(theme::HEADING).size(18).color(theme::TEXT)]
            .extend(rows)
            .spacing(16),
    )
    .padding(22)
    .width(Length::Fill)
    .style(theme::card(20.0))
    .into()
}

fn list<'a, T: Clone + PartialEq + 'a>(
    choices: Vec<Choice<T>>,
    current: &T,
    on: impl Fn(T) -> SettingChange + 'a,
) -> Element<'a, Message> {
    let selected = choices.iter().find(|c| &c.value == current).cloned();
    pick_list(choices, selected, move |c: Choice<T>| {
        Message::Setting(on(c.value))
    })
    .width(280)
    .padding([10, 14])
    .style(theme::pick)
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let s = &app.settings;
    let since = app.vad_changed.elapsed().as_secs_f32();

    let mut languages = vec![Choice {
        value: None,
        label: "Auto".to_string(),
    }];
    let mut named: Vec<_> = verbacue::translate::languages()
        .iter()
        .map(|(code, name)| Choice {
            value: Some((*code).to_string()),
            label: (*name).to_string(),
        })
        .collect();
    named.sort_by(|a, b| a.label.cmp(&b.label));
    languages.extend(named);

    let speech = card(
        "Speech to text",
        vec![
            setting_row(
                "Whisper model",
                "Bigger is more accurate and slower.",
                list(
                    model_choices(Kind::Whisper),
                    &s.whisper_model,
                    SettingChange::WhisperModel,
                ),
            ),
            setting_row(
                "Spoken language",
                "Whisper detects it when you choose Auto.",
                list(languages, &s.language, SettingChange::Language),
            ),
            setting_row(
                "Skip silence and music",
                "Stops Whisper from inventing text.",
                switch::view(s.vad, since, Message::Setting(SettingChange::Vad(!s.vad))),
            ),
        ],
    );

    let segment = |label: &'static str, backend: Backend| {
        let on = s.translator == backend;
        button(text(label).font(theme::BODY_MEDIUM).size(14))
            .padding([10, 16])
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: on.then(|| theme::ACCENT.into()),
                    text_color: if on {
                        theme::ON_ACCENT
                    } else if hovered {
                        theme::TEXT
                    } else {
                        theme::TEXT_SOFT
                    },
                    border: iced::Border {
                        radius: 9.0.into(),
                        ..iced::Border::default()
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::Setting(SettingChange::Translator(backend)))
    };
    let more = vec![
        Choice {
            value: Backend::Azure,
            label: "Azure".to_string(),
        },
        Choice {
            value: Backend::None,
            label: "No translation".to_string(),
        },
    ];
    let more_selected = more.iter().find(|c| c.value == s.translator).cloned();
    let translators = row![
        container(
            row![
                segment("Local", Backend::Local),
                segment("DeepL", Backend::Deepl),
                segment("Google", Backend::Google),
                segment("Server", Backend::OpenaiCompat),
            ]
            .spacing(4)
        )
        .padding(4)
        .style(|_| container::Style {
            background: Some(iced::Color::from_rgb8(0x1f, 0x17, 0x14).into()),
            border: iced::Border {
                radius: 12.0.into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        }),
        pick_list(more, more_selected, |c: Choice<Backend>| {
            Message::Setting(SettingChange::Translator(c.value))
        })
        .placeholder("More")
        .padding([10, 14])
        .style(theme::pick),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut translation = vec![setting_row(
        "Translator",
        "Local runs offline. Online services need a key.",
        translators,
    )];
    if let Some((var, file)) = key_source(s.translator) {
        let found = key_found(var, file);
        let status = if found {
            text("Key found").size(13).color(theme::SUCCESS_TEXT)
        } else {
            text(format!("No key: set {var} or save it in {file}"))
                .size(13)
                .color(theme::ACCENT_TEXT)
        };
        translation.push(setting_row(
            "Service key",
            "verbacue reads it from the environment or the config folder.",
            row![
                status,
                button(text("Open config folder").size(13))
                    .padding([9, 14])
                    .style(theme::ghost)
                    .on_press(Message::OpenConfigFolder),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ));
    }
    if shows_server_fields(s) {
        translation.push(setting_row(
            "Server URL",
            "An OpenAI-compatible chat API.",
            text_input("http://localhost:8080/v1", &s.endpoint)
                .on_input(|v| Message::Setting(SettingChange::Endpoint(v)))
                .padding([10, 14])
                .width(320),
        ));
        translation.push(setting_row(
            "Server model",
            "Empty: the first model that the server lists.",
            text_input("", s.endpoint_model.as_deref().unwrap_or(""))
                .on_input(|v| Message::Setting(SettingChange::EndpointModel(v)))
                .padding([10, 14])
                .width(320),
        ));
    }
    if s.translator == Backend::Local {
        translation.push(setting_row(
            "Language model",
            "Used by the local translator.",
            list(
                model_choices(Kind::Llm),
                &s.llm_model,
                SettingChange::LlmModel,
            ),
        ));
    }
    translation.push(setting_row(
        "Form of address",
        "Formal or informal \"you\".",
        list(
            vec![
                Choice {
                    value: Register::Informal,
                    label: "Informal".into(),
                },
                Choice {
                    value: Register::Formal,
                    label: "Formal".into(),
                },
            ],
            &s.register,
            SettingChange::Register,
        ),
    ));
    let glossary_label = s.glossary.as_ref().map_or_else(
        || "Choose a file…".to_string(),
        |p| {
            p.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        },
    );
    let mut glossary = row![
        button(text(glossary_label).size(13))
            .padding([9, 14])
            .style(theme::ghost)
            .on_press(Message::ChooseGlossary)
    ]
    .spacing(8);
    if s.glossary.is_some() {
        glossary = glossary.push(
            button(text("Remove").size(13))
                .padding([9, 14])
                .style(theme::ghost)
                .on_press(Message::Setting(SettingChange::Glossary(None))),
        );
    }
    if matches!(s.translator, Backend::Local | Backend::OpenaiCompat) {
        translation.push(setting_row(
            "Read the whole story first",
            "Notes about the characters before translating. Slower; better names and forms of address.",
            switch::view(
                s.film_notes,
                app.film_notes_changed.elapsed().as_secs_f32(),
                Message::Setting(SettingChange::FilmNotes(!s.film_notes)),
            ),
        ));
    }
    translation.push(setting_row(
        "Glossary",
        "Fixed translations for names and terms.",
        glossary,
    ));
    let translation = card("Translation", translation);

    let out_label = s.out_dir.as_ref().map_or_else(
        || "Next to the video".to_string(),
        |p| p.display().to_string(),
    );
    let mut save_to = row![
        button(text(out_label).size(13))
            .padding([9, 14])
            .style(theme::ghost)
            .on_press(Message::ChooseOutDir)
    ]
    .spacing(8);
    if s.out_dir.is_some() {
        save_to = save_to.push(
            button(text("Next to the video").size(13))
                .padding([9, 14])
                .style(theme::ghost)
                .on_press(Message::Setting(SettingChange::OutDir(None))),
        );
    }
    let output = card(
        "Output",
        vec![
            setting_row(
                "Subtitle format",
                "For the separate file.",
                list(
                    vec![
                        Choice {
                            value: Format::Srt,
                            label: "SRT".into(),
                        },
                        Choice {
                            value: Format::Vtt,
                            label: "WebVTT".into(),
                        },
                    ],
                    &s.format,
                    SettingChange::Format,
                ),
            ),
            setting_row(
                "Save to",
                "Next to the video, or a folder you choose.",
                save_to,
            ),
        ],
    );

    let model_rows = models::known()
        .iter()
        .map(|m| {
            let downloaded = models::cached_path(m).is_ok_and(|p| p.is_file());
            let kind = match m.kind {
                Kind::Whisper => "Whisper",
                Kind::Llm => "Translation",
                Kind::Vad => "Silence detection",
            };
            let hint = format!("{kind} · {}", models::human_size(m.size));
            let control: Element<'_, Message> = match app.downloads.get(m.name) {
                Some(&(done, total)) => text(format!(
                    "Downloading {} %",
                    (done * 100).checked_div(total).unwrap_or(0)
                ))
                .size(13)
                .color(theme::ACCENT_TEXT)
                .into(),
                None if downloaded => row![
                    container(text("Downloaded").size(12).color(theme::SUCCESS_TEXT))
                        .padding([4, 10]),
                    button(text("Remove").size(13))
                        .padding([9, 14])
                        .style(theme::ghost)
                        .on_press(Message::RemoveModel(m.name)),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
                None => button(text("Download").size(13).color(theme::ACCENT_TEXT))
                    .padding([9, 14])
                    .style(theme::ghost)
                    .on_press(Message::DownloadModel(m.name))
                    .into(),
            };
            row![
                column![
                    text(m.name).font(theme::BODY_BOLD).color(theme::TEXT),
                    text(hint).size(13).color(theme::MUTED),
                ]
                .spacing(3)
                .width(Length::Fill),
                control,
            ]
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    let models_card = card("Models on this computer", model_rows);

    scrollable(
        container(
            column![
                text("Settings")
                    .font(theme::HEADING)
                    .size(36)
                    .color(theme::TEXT),
                speech,
                translation,
                output,
                models_card,
            ]
            .spacing(20)
            .max_width(900),
        )
        .padding([40, 56]),
    )
    .into()
}
