use gpui::Action;

/// Quit the application.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = rtoken, no_json)]
pub struct Quit;
