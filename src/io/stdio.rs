//! Standard I/O Implementation
//!
//! This module provides standard input/output operations with
//! Chinese language support and console interface management.

use super::{IoError, IoResult};
use std::io::{self, Read, Write};

/// Standard I/O interface
#[derive(Debug)]
pub struct StandardIo {
    /// Input buffer
    input_buffer: String,
    /// Output buffer
    output_buffer: String,
    /// Error buffer
    error_buffer: String,
    /// Use buffering
    use_buffering: bool,
}

impl StandardIo {
    /// Create new standard I/O interface
    pub fn new() -> Self {
        Self {
            input_buffer: String::new(),
            output_buffer: String::new(),
            error_buffer: String::new(),
            use_buffering: true,
        }
    }

    /// Create standard I/O with buffering disabled
    pub fn unbuffered() -> Self {
        Self {
            input_buffer: String::new(),
            output_buffer: String::new(),
            error_buffer: String::new(),
            use_buffering: false,
        }
    }

    /// Read a line from standard input
    pub fn read_line(&mut self) -> IoResult<String> {
        let mut input = String::new();

        io::stdin()
            .read_line(&mut input)
            .map_err(|e| IoError::SystemIoError(e))?;

        // Remove trailing newline
        if input.ends_with('\n') {
            input.pop();
            if input.ends_with('\r') {
                input.pop();
            }
        }

        Ok(input)
    }

    /// Read all input from standard input
    pub fn read_all(&mut self) -> IoResult<String> {
        let mut input = String::new();

        io::stdin()
            .read_to_string(&mut input)
            .map_err(|e| IoError::SystemIoError(e))?;

        Ok(input)
    }

    /// Print to standard output
    pub fn print(&mut self, text: &str) -> IoResult<()> {
        if self.use_buffering {
            self.output_buffer.push_str(text);
            Ok(())
        } else {
            io::stdout()
                .write_all(text.as_bytes())
                .map_err(|e| IoError::SystemIoError(e))
        }
    }

    /// Print to standard output with newline
    pub fn println(&mut self, text: &str) -> IoResult<()> {
        if self.use_buffering {
            self.output_buffer.push_str(text);
            self.output_buffer.push('\n');
            Ok(())
        } else {
            writeln!(io::stdout(), "{}", text).map_err(|e| IoError::SystemIoError(e))
        }
    }

    /// Print to standard error
    pub fn eprint(&mut self, text: &str) -> IoResult<()> {
        if self.use_buffering {
            self.error_buffer.push_str(text);
            Ok(())
        } else {
            io::stderr()
                .write_all(text.as_bytes())
                .map_err(|e| IoError::SystemIoError(e))
        }
    }

    /// Print to standard error with newline
    pub fn eprintln(&mut self, text: &str) -> IoResult<()> {
        if self.use_buffering {
            self.error_buffer.push_str(text);
            self.error_buffer.push('\n');
            Ok(())
        } else {
            writeln!(io::stderr(), "{}", text).map_err(|e| IoError::SystemIoError(e))
        }
    }

    /// Print integer value
    pub fn print_int(&mut self, value: i64) -> IoResult<()> {
        self.print(&value.to_string())
    }

    /// Print integer value with newline
    pub fn println_int(&mut self, value: i64) -> IoResult<()> {
        self.println(&value.to_string())
    }

    /// Print floating point value
    pub fn print_float(&mut self, value: f64) -> IoResult<()> {
        self.print(&value.to_string())
    }

    /// Print floating point value with newline
    pub fn println_float(&mut self, value: f64) -> IoResult<()> {
        self.println(&value.to_string())
    }

    /// Print boolean value
    pub fn print_bool(&mut self, value: bool) -> IoResult<()> {
        let text = if value { "真" } else { "假" };
        self.print(text)
    }

    /// Print boolean value with newline
    pub fn println_bool(&mut self, value: bool) -> IoResult<()> {
        let text = if value { "真" } else { "假" };
        self.println(text)
    }

    /// Flush output buffer
    pub fn flush_output(&mut self) -> IoResult<()> {
        if !self.output_buffer.is_empty() {
            io::stdout()
                .write_all(self.output_buffer.as_bytes())
                .map_err(|e| IoError::SystemIoError(e))?;

            io::stdout()
                .flush()
                .map_err(|e| IoError::SystemIoError(e))?;

            self.output_buffer.clear();
        }
        Ok(())
    }

    /// Flush error buffer
    pub fn flush_error(&mut self) -> IoResult<()> {
        if !self.error_buffer.is_empty() {
            io::stderr()
                .write_all(self.error_buffer.as_bytes())
                .map_err(|e| IoError::SystemIoError(e))?;

            io::stderr()
                .flush()
                .map_err(|e| IoError::SystemIoError(e))?;

            self.error_buffer.clear();
        }
        Ok(())
    }

    /// Flush all buffers
    pub fn flush_all(&mut self) -> IoResult<()> {
        self.flush_output()?;
        self.flush_error()
    }

    /// Get output buffer content
    pub fn get_output_buffer(&self) -> &str {
        &self.output_buffer
    }

    /// Get error buffer content
    pub fn get_error_buffer(&self) -> &str {
        &self.error_buffer
    }

    /// Clear all buffers
    pub fn clear_buffers(&mut self) {
        self.output_buffer.clear();
        self.error_buffer.clear();
    }

    /// Check if buffering is enabled
    pub fn is_buffered(&self) -> bool {
        self.use_buffering
    }

    /// Set buffering mode
    pub fn set_buffering(&mut self, enabled: bool) {
        self.use_buffering = enabled;
    }
}

impl Default for StandardIo {
    fn default() -> Self {
        Self::new()
    }
}

/// Console interface with enhanced functionality
#[derive(Debug)]
pub struct ConsoleInterface {
    /// Standard I/O
    stdio: StandardIo,
    /// Console width (if detectable)
    width: Option<usize>,
    /// Console height (if detectable)
    height: Option<usize>,
    /// Use colors
    use_colors: bool,
}

/// Console colors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl ConsoleColor {
    /// Get ANSI color code
    pub fn ansi_code(self) -> &'static str {
        match self {
            ConsoleColor::Black => "30",
            ConsoleColor::Red => "31",
            ConsoleColor::Green => "32",
            ConsoleColor::Yellow => "33",
            ConsoleColor::Blue => "34",
            ConsoleColor::Magenta => "35",
            ConsoleColor::Cyan => "36",
            ConsoleColor::White => "37",
            ConsoleColor::BrightBlack => "90",
            ConsoleColor::BrightRed => "91",
            ConsoleColor::BrightGreen => "92",
            ConsoleColor::BrightYellow => "93",
            ConsoleColor::BrightBlue => "94",
            ConsoleColor::BrightMagenta => "95",
            ConsoleColor::BrightCyan => "96",
            ConsoleColor::BrightWhite => "97",
        }
    }

    /// Get background ANSI color code
    pub fn ansi_bg_code(self) -> &'static str {
        match self {
            ConsoleColor::Black => "40",
            ConsoleColor::Red => "41",
            ConsoleColor::Green => "42",
            ConsoleColor::Yellow => "43",
            ConsoleColor::Blue => "44",
            ConsoleColor::Magenta => "45",
            ConsoleColor::Cyan => "46",
            ConsoleColor::White => "47",
            ConsoleColor::BrightBlack => "100",
            ConsoleColor::BrightRed => "101",
            ConsoleColor::BrightGreen => "102",
            ConsoleColor::BrightYellow => "103",
            ConsoleColor::BrightBlue => "104",
            ConsoleColor::BrightMagenta => "105",
            ConsoleColor::BrightCyan => "106",
            ConsoleColor::BrightWhite => "107",
        }
    }
}

impl ConsoleInterface {
    /// Create new console interface
    pub fn new() -> Self {
        let (width, height) = Self::detect_console_size();

        Self {
            stdio: StandardIo::new(),
            width,
            height,
            use_colors: Self::detect_color_support(),
        }
    }

    /// Create console interface with specific settings
    pub fn with_settings(buffered: bool, colors: bool) -> Self {
        let mut console = Self::new();
        console.stdio.set_buffering(buffered);
        console.use_colors = colors;
        console
    }

    /// Print text with color
    pub fn print_color(&mut self, text: &str, color: ConsoleColor) -> IoResult<()> {
        if self.use_colors {
            let colored_text = format!("\x1b[{}m{}\x1b[0m", color.ansi_code(), text);
            self.stdio.print(&colored_text)
        } else {
            self.stdio.print(text)
        }
    }

    /// Print text with background color
    pub fn print_bg_color(&mut self, text: &str, color: ConsoleColor) -> IoResult<()> {
        if self.use_colors {
            let colored_text = format!("\x1b[{}m{}\x1b[0m", color.ansi_bg_code(), text);
            self.stdio.print(&colored_text)
        } else {
            self.stdio.print(text)
        }
    }

    /// Print colored text with newline
    pub fn println_color(&mut self, text: &str, color: ConsoleColor) -> IoResult<()> {
        self.print_color(text, color)?;
        self.stdio.println("")
    }

    /// Print error message in red
    pub fn print_error(&mut self, text: &str) -> IoResult<()> {
        self.println_color(text, ConsoleColor::Red)
    }

    /// Print success message in green
    pub fn print_success(&mut self, text: &str) -> IoResult<()> {
        self.println_color(text, ConsoleColor::Green)
    }

    /// Print warning message in yellow
    pub fn print_warning(&mut self, text: &str) -> IoResult<()> {
        self.println_color(text, ConsoleColor::Yellow)
    }

    /// Print info message in blue
    pub fn print_info(&mut self, text: &str) -> IoResult<()> {
        self.println_color(text, ConsoleColor::Blue)
    }

    /// Print header with underline
    pub fn print_header(&mut self, text: &str) -> IoResult<()> {
        self.println_color(text, ConsoleColor::BrightCyan)?;
        let underline = "=".repeat(text.len());
        self.println_color(&underline, ConsoleColor::Cyan)
    }

    /// Print text with newline
    pub fn println(&mut self, text: &str) -> IoResult<()> {
        self.stdio.println(text)
    }

    /// Print separator line
    pub fn print_separator(&mut self) -> IoResult<()> {
        let width = self.width.unwrap_or(80);
        let separator = "=".repeat(width);
        self.println(&separator)
    }

    /// Get console width
    pub fn width(&self) -> Option<usize> {
        self.width
    }

    /// Get console height
    pub fn height(&self) -> Option<usize> {
        self.height
    }

    /// Check if colors are supported
    pub fn supports_colors(&self) -> bool {
        self.use_colors
    }

    /// Clear screen
    pub fn clear_screen(&mut self) -> IoResult<()> {
        self.stdio.print("\x1b[2J\x1b[H")
    }

    /// Move cursor to position
    pub fn move_cursor(&mut self, x: u16, y: u16) -> IoResult<()> {
        self.stdio.print(&format!("\x1b[{};{}H", y, x))
    }

    /// Hide cursor
    pub fn hide_cursor(&mut self) -> IoResult<()> {
        self.stdio.print("\x1b[?25l")
    }

    /// Show cursor
    pub fn show_cursor(&mut self) -> IoResult<()> {
        self.stdio.print("\x1b[?25h")
    }

    /// Detect console size
    fn detect_console_size() -> (Option<usize>, Option<usize>) {
        #[cfg(unix)]
        {
            // Use ioctl on Unix systems
            unsafe {
                use libc::{ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ};

                let mut size: winsize = std::mem::zeroed();
                if ioctl(STDOUT_FILENO, TIOCGWINSZ, &mut size) == 0 {
                    (Some(size.ws_col as usize), Some(size.ws_row as usize))
                } else {
                    (None, None)
                }
            }
        }
        #[cfg(not(unix))]
        {
            // Fallback for non-Unix systems
            (None, None)
        }
    }

    /// Detect color support
    fn detect_color_support() -> bool {
        // Check if terminal supports colors
        std::env::var("TERM")
            .map(|term| {
                term.contains("color")
                    || term.contains("256")
                    || term.contains("xterm")
                    || term.contains("screen")
            })
            .unwrap_or(false)
    }
}

impl Default for ConsoleInterface {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for StandardIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        self.print(&s)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "I/O error"))?;
        Ok(s.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_all()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "I/O error"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_io_creation() {
        let io = StandardIo::new();
        assert!(io.is_buffered());

        let io = StandardIo::unbuffered();
        assert!(!io.is_buffered());
    }

    #[test]
    fn test_standard_io_printing() {
        let mut io = StandardIo::unbuffered();

        let result = io.print("Hello");
        assert!(result.is_ok());

        let result = io.println("World");
        assert!(result.is_ok());
    }

    #[test]
    fn test_standard_io_types() {
        let mut io = StandardIo::unbuffered();

        let result = io.print_int(42);
        assert!(result.is_ok());

        let result = io.println_float(3.14);
        assert!(result.is_ok());

        let result = io.print_bool(true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_standard_io_buffering() {
        let mut io = StandardIo::new();

        // With buffering, nothing should be printed yet
        io.print("Buffered").unwrap();
        assert_eq!(io.get_output_buffer(), "Buffered");

        // Flush should clear the buffer
        io.flush_output().unwrap();
        assert_eq!(io.get_output_buffer(), "");
    }

    #[test]
    fn test_console_colors() {
        assert_eq!(ConsoleColor::Red.ansi_code(), "31");
        assert_eq!(ConsoleColor::Green.ansi_code(), "32");
        assert_eq!(ConsoleColor::Red.ansi_bg_code(), "41");
        assert_eq!(ConsoleColor::Green.ansi_bg_code(), "42");

        assert_ne!(ConsoleColor::Red, ConsoleColor::Blue);
    }

    #[test]
    fn test_console_interface() {
        let console = ConsoleInterface::new();

        // These should not panic
        let _ = console.width();
        let _ = console.height();
        let _ = console.supports_colors();
    }

    #[test]
    fn test_console_printing() {
        let mut console = ConsoleInterface::with_settings(false, false);

        let result = console.print_info("Info message");
        assert!(result.is_ok());

        let result = console.print_error("Error message");
        assert!(result.is_ok());

        let result = console.print_success("Success message");
        assert!(result.is_ok());
    }
}
