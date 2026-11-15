//! The verbacue installer. It finds the build that fits this computer (system,
//! processor, GPU), lets the user change it, then downloads the latest release file,
//! checks its SHA256 and installs it without admin rights.

// No console window next to the installer window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod choose;
mod detect;
mod error;
mod install;
mod release;

use iced::{
    Alignment, Color, Element, Length, Size, Task, Theme,
    futures::{Stream, stream},
    widget::{button, column, container, pick_list, progress_bar, row, text},
};

use crate::{
    choose::{Build, Choice},
    detect::{Facts, Os},
    install::Installed,
    release::Release,
};

const ACCENT: Color = Color::from_rgb8(0xe8, 0x62, 0x2a);
const MUTED: Color = Color::from_rgb8(0xa8, 0x9a, 0x8d);

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Install verbacue")
        .theme(|_: &App| {
            Theme::custom(
                "verbacue".to_string(),
                iced::theme::Palette {
                    background: Color::from_rgb8(0x12, 0x0e, 0x0d),
                    text: Color::from_rgb8(0xf3, 0xe9, 0xdc),
                    primary: ACCENT,
                    ..iced::theme::Palette::DARK
                },
            )
        })
        .window_size(Size::new(640.0, 600.0))
        .run()
}

#[derive(Debug, Clone)]
enum Stage {
    /// Reading the computer and the latest release.
    Looking,
    Ready,
    Downloading {
        done: u64,
        total: u64,
    },
    Installing,
    Done(Installed),
    Failed(String),
}

struct App {
    facts: Option<Facts>,
    release: Result<Release, String>,
    build: Option<Build>,
    stage: Stage,
}

#[derive(Debug, Clone)]
enum Message {
    Found(Box<(Facts, Result<Release, String>)>),
    Pick(Pick),
    Install,
    Progress(u64, u64),
    Installing,
    Finished(Result<Installed, String>),
    Open,
    Close,
}

/// A build in the list, marked when it is the recommended one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pick {
    build: Build,
    recommended: bool,
}

impl std::fmt::Display for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.recommended {
            write!(f, "{} (recommended)", self.build)
        } else {
            write!(f, "{}", self.build)
        }
    }
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self {
            facts: None,
            release: Err(String::new()),
            build: None,
            stage: Stage::Looking,
        };
        let look = Task::perform(
            async {
                tokio::task::spawn_blocking(|| {
                    (
                        detect::facts(),
                        release::latest().map_err(|e| e.to_string()),
                    )
                })
                .await
                .expect("the detection thread stopped")
            },
            |found| Message::Found(Box::new(found)),
        );
        (app, look)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Found(found) => {
                let (facts, release) = *found;
                self.build = choose::recommend(&facts);
                self.facts = Some(facts);
                self.release = release;
                self.stage = Stage::Ready;
                Task::none()
            }
            Message::Pick(pick) => {
                self.build = Some(pick.build);
                Task::none()
            }
            Message::Install => match self.download_job() {
                Ok((release, asset)) => {
                    self.stage = Stage::Downloading {
                        done: 0,
                        total: asset.size,
                    };
                    Task::run(run_install(release, asset), |m| m)
                }
                Err(e) => {
                    self.stage = Stage::Failed(e);
                    Task::none()
                }
            },
            Message::Progress(done, total) => {
                self.stage = Stage::Downloading { done, total };
                Task::none()
            }
            Message::Installing => {
                self.stage = Stage::Installing;
                Task::none()
            }
            Message::Finished(result) => {
                self.stage = match result {
                    Ok(installed) => Stage::Done(installed),
                    Err(e) => Stage::Failed(e),
                };
                Task::none()
            }
            Message::Open => {
                if let Stage::Done(Installed::App(app)) = &self.stage {
                    install::open(app);
                }
                iced::exit()
            }
            Message::Close => iced::exit(),
        }
    }

    /// The release and the file to download for the chosen build.
    fn download_job(&self) -> Result<(Release, release::Asset), String> {
        let release = self.release.clone()?;
        let facts = self
            .facts
            .as_ref()
            .ok_or("the computer is not checked yet")?;
        let build = self.build.ok_or("pick a build first")?;
        let name = choose::asset_name(build, facts, release.version())?;
        let asset = release
            .asset(&name)
            .cloned()
            .ok_or_else(|| format!("release {} has no {name}", release.tag_name))?;
        Ok((release, asset))
    }

    fn view(&self) -> Element<'_, Message> {
        let mut page = column![
            text("Install verbacue").size(32),
            text("Subtitles and dubbing for any video, made on this computer.").color(MUTED),
        ]
        .spacing(8);

        let Some(facts) = &self.facts else {
            page = page.push(text("Checking this computer and the latest release…"));
            return frame(page);
        };
        page = page.push(facts_card(facts));

        match &self.release {
            Ok(release) => {
                page =
                    page.push(text(format!("Latest version: {}", release.version())).color(MUTED));
            }
            Err(e) => {
                page = page.push(text(e.clone()).color(ACCENT));
                return frame(page);
            }
        }

        let choices = choose::choices(facts);
        page = page.push(build_picker(&choices, self.build, choose::recommend(facts)));
        page = page.push(self.action(facts));
        frame(page)
    }

    /// The install button, the progress, or the result.
    fn action(&self, facts: &Facts) -> Element<'_, Message> {
        match &self.stage {
            Stage::Looking => text("").into(),
            Stage::Ready => {
                let size = self
                    .download_job()
                    .map(|(_, asset)| format!("Download: {}", megabytes(asset.size)));
                column![
                    button(text("Install").size(18))
                        .padding([12, 28])
                        .on_press_maybe(size.is_ok().then_some(Message::Install)),
                    text(size.unwrap_or_else(|e| e)).size(14).color(MUTED),
                ]
                .spacing(8)
                .into()
            }
            Stage::Downloading { done, total } => {
                #[expect(clippy::cast_precision_loss, reason = "a progress bar")]
                let part = if *total == 0 {
                    0.0
                } else {
                    *done as f32 / *total as f32
                };
                column![
                    text(format!(
                        "Downloading… {} of {}",
                        megabytes(*done),
                        megabytes(*total)
                    )),
                    progress_bar(0.0..=1.0, part),
                ]
                .spacing(8)
                .into()
            }
            Stage::Installing => text(installing_text(facts.os)).into(),
            Stage::Done(Installed::App(path)) => column![
                text("verbacue is installed.").size(20),
                text(path.display().to_string()).size(14).color(MUTED),
                row![
                    button(text("Open verbacue"))
                        .padding([10, 22])
                        .on_press(Message::Open),
                    button(text("Close"))
                        .padding([10, 22])
                        .style(button::secondary)
                        .on_press(Message::Close),
                ]
                .spacing(12),
            ]
            .spacing(10)
            .into(),
            Stage::Done(Installed::CommandLine(path)) => column![
                text("verbacue is installed.").size(20),
                text(format!(
                    "Run \"verbacue --help\" in a terminal. The program is {}. \
                     If the terminal does not find it, add ~/.local/bin to PATH.",
                    path.display()
                ))
                .size(14)
                .color(MUTED),
                button(text("Close"))
                    .padding([10, 22])
                    .on_press(Message::Close),
            ]
            .spacing(10)
            .into(),
            Stage::Failed(e) => column![
                text("The install failed.").size(20).color(ACCENT),
                text(e.clone()).size(14).color(MUTED),
                button(text("Try again"))
                    .padding([10, 22])
                    .on_press(Message::Install),
            ]
            .spacing(10)
            .into(),
        }
    }
}

fn frame(page: iced::widget::Column<'_, Message>) -> Element<'_, Message> {
    container(page.spacing(18).max_width(560))
        .padding(32)
        .width(Length::Fill)
        .into()
}

/// What the installer found: system, GPU and Vulkan.
fn facts_card(facts: &Facts) -> Element<'_, Message> {
    let system = match (facts.os, facts.glibc) {
        (Os::Linux, Some((major, minor))) => {
            format!(
                "Linux {} with glibc {major}.{minor}",
                std::env::consts::ARCH
            )
        }
        (Os::Linux, None) => format!("Linux {} without glibc", std::env::consts::ARCH),
        (Os::Mac, _) => format!("macOS {}", std::env::consts::ARCH),
        (Os::Windows, _) => format!("Windows {}", std::env::consts::ARCH),
        (Os::Other, _) => std::env::consts::OS.to_string(),
    };
    let nvidia = match &facts.nvidia {
        Some((name, (major, minor))) => format!("{name}, driver {major}.{minor}"),
        None => "none found".to_string(),
    };
    let line = |label: &'static str, value: String| {
        row![text(label).width(120).color(MUTED), text(value),].spacing(12)
    };
    let mut card = column![line("System", system)].spacing(6);
    if facts.os != Os::Mac {
        card = card.push(line("NVIDIA GPU", nvidia)).push(line(
            "Vulkan",
            if facts.vulkan { "found" } else { "not found" }.into(),
        ));
    }
    container(card)
        .padding(16)
        .width(Length::Fill)
        .style(container::rounded_box)
        .into()
}

/// The list of builds that fit, and why the others do not.
fn build_picker(
    choices: &[Choice],
    selected: Option<Build>,
    recommended: Option<Build>,
) -> Element<'static, Message> {
    let picks: Vec<Pick> = choices
        .iter()
        .filter(|c| c.fits.is_ok())
        .map(|c| Pick {
            build: c.build,
            recommended: Some(c.build) == recommended,
        })
        .collect();
    let current = picks.iter().find(|p| Some(p.build) == selected).cloned();
    let mut col = column![
        row![
            text("Build").width(120).color(MUTED),
            pick_list(picks, current, Message::Pick).width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
    ]
    .spacing(6);
    for choice in choices {
        if let Err(reason) = &choice.fits {
            col = col.push(
                text(format!("Not available: {}. {reason}", choice.build))
                    .size(13)
                    .color(MUTED),
            );
        }
    }
    col.into()
}

fn installing_text(os: Os) -> &'static str {
    match os {
        Os::Windows => "Installing… The setup window shows the progress.",
        _ => "Installing…",
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes / 1_000_000)
}

/// Downloads and installs on a thread, and reports the steps as messages.
fn run_install(release: Release, asset: release::Asset) -> impl Stream<Item = Message> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let dir = std::env::temp_dir().join("verbacue-installer");
        let progress = |done, total| {
            let _ = sender.send(Message::Progress(done, total));
        };
        let result = release::download(&release, &asset, &dir, &progress).and_then(|file| {
            let _ = sender.send(Message::Installing);
            let installed = install::install(&file);
            let _ = std::fs::remove_file(&file);
            installed
        });
        let _ = sender.send(Message::Finished(result.map_err(|e| e.to_string())));
    });
    stream::poll_fn(move |cx| receiver.poll_recv(cx))
}
