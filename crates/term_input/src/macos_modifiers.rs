/// macOS-specific: query live keyboard modifier state via CoreGraphics.
///
/// Allows detecting Option/Alt and Shift keys even when the terminal emulator
/// (e.g. Warp in alternate screen mode) does not send ESC prefix or
/// kitty keyboard protocol sequences.

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
    /// NX_SHIFTMASK / kCGEventFlagMaskShift
    const FLAG_MASK_SHIFT: u64 = 0x00020000;

    fn flags() -> u64 {
        unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) }
    }

    /// Returns `true` if either Option/Alt key is currently held down.
    pub fn is_option_held() -> bool {
        (flags() & FLAG_MASK_ALTERNATE) != 0
    }

    /// Returns `true` if either Shift key is currently held down.
    pub fn is_shift_held() -> bool {
        (flags() & FLAG_MASK_SHIFT) != 0
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

/// Returns `true` if the Shift key is currently held down.
/// On non-macOS platforms, always returns `false`.
pub fn is_shift_held() -> bool {
    #[cfg(target_os = "macos")]
    {
        cg::is_shift_held()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
