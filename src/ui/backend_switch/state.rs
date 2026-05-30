use crate::ui::backend_switch::intent::BackendSwitchIntent;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum BackendPopupSection {
    #[default]
    ActiveBackend,
    SubagentBackend,
    TeammateBackend,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum BackendSwitchState {
    #[default]
    Hidden,
    Visible {
        section: BackendPopupSection,
        backend_selection: usize,
        subagent_selection: usize,
        teammate_selection: usize,
        /// Number of backends (needed for wrap-around navigation).
        backends_count: usize,
    },
}

impl BackendSwitchState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// The single authoritative transition (the plain-fn replacement for the
    /// old MVI `Actor::handle_intent` — same semantics, mutated in place).
    pub fn apply(&mut self, intent: BackendSwitchIntent) {
        match intent {
            BackendSwitchIntent::Open {
                backend_selection,
                subagent_selection,
                teammate_selection,
                backends_count,
            } => {
                *self = BackendSwitchState::Visible {
                    section: BackendPopupSection::ActiveBackend,
                    backend_selection,
                    subagent_selection,
                    teammate_selection,
                    backends_count,
                };
            }
            BackendSwitchIntent::Close => *self = BackendSwitchState::Hidden,
            BackendSwitchIntent::NextSection => {
                if let BackendSwitchState::Visible { section, .. } = self {
                    *section = match *section {
                        BackendPopupSection::ActiveBackend => BackendPopupSection::SubagentBackend,
                        BackendPopupSection::SubagentBackend => {
                            BackendPopupSection::TeammateBackend
                        }
                        BackendPopupSection::TeammateBackend => BackendPopupSection::ActiveBackend,
                    };
                }
            }
            BackendSwitchIntent::MoveUp => self.navigate(-1),
            BackendSwitchIntent::MoveDown => self.navigate(1),
            BackendSwitchIntent::Clear => {
                if let BackendSwitchState::Visible {
                    section,
                    subagent_selection,
                    teammate_selection,
                    ..
                } = self
                {
                    // Active backend cannot be cleared (the proxy always has
                    // one); only the override sections reset to Disabled (0).
                    match section {
                        BackendPopupSection::ActiveBackend => {}
                        BackendPopupSection::SubagentBackend => *subagent_selection = 0,
                        BackendPopupSection::TeammateBackend => *teammate_selection = 0,
                    }
                }
            }
        }
    }

    fn navigate(&mut self, direction: i32) {
        if let BackendSwitchState::Visible {
            section,
            backend_selection,
            subagent_selection,
            teammate_selection,
            backends_count,
        } = self
        {
            match section {
                BackendPopupSection::ActiveBackend => {
                    *backend_selection = wrap_around(*backend_selection, direction, *backends_count);
                }
                BackendPopupSection::SubagentBackend => {
                    let total = *backends_count + 1;
                    *subagent_selection = wrap_around(*subagent_selection, direction, total);
                }
                BackendPopupSection::TeammateBackend => {
                    let total = *backends_count + 1;
                    *teammate_selection = wrap_around(*teammate_selection, direction, total);
                }
            }
        }
    }
}

fn wrap_around(current: usize, direction: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let current = current.min(len - 1);
    if direction.is_negative() {
        if current == 0 {
            len - 1
        } else {
            current - 1
        }
    } else if current + 1 >= len {
        0
    } else {
        current + 1
    }
}
