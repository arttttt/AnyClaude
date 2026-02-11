/// macOS-specific: query live keyboard modifier state via CoreGraphics.
///
/// Allows detecting Option/Alt key even when the terminal emulator
/// (e.g. Warp in alternate screen mode) does not send ESC prefix.

#[cfg(target_os = "macos")]
mod cg {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(stateID: u32) -> u64;
    }

    /// kCGEventSourceStateCombinedSessionState
    const COMBINED_SESSION_STATE: u32 = 0;
    /// NX_ALTERNATEMASK / kCGEventFlagMaskAlternate
    const FLAG_MASK_ALTERNATE: u64 = 0x00080000;

    /// Returns `true` if either Option/Alt key is currently held down.
    pub fn is_option_held() -> bool {
        let flags = unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) };
        (flags & FLAG_MASK_ALTERNATE) != 0
    }
}

/// Returns `true` if the Option/Alt key is currently held down.
/// On non-macOS platforms, always returns `false`.
pub fn is_option_held() -> bool {
    #[cfg(target_os = "macos")]
    {
        cg::is_option_held()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
