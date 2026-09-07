use serde::Deserialize;
/// One bounded JSON object per line. The model cannot create or execute these events.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Task {
    Status,
    Ask {
        text: String,
        #[serde(default)]
        complex: bool,
    },
    Email {
        path: std::path::PathBuf,
    },
    Research {
        query: String,
    },
    Discord {
        channel: String,
    },
    Speak {
        text: String,
    },
    Listen,
    Quit,
}
