// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_core::error::AnyError;
use deno_core::op;
use deno_core::OpState;
use deno_io::StdFileResource;
use std::io::Error;

#[cfg(unix)]
use nix::sys::termios;

#[cfg(windows)]
use deno_core::error::custom_error;
#[cfg(windows)]
use winapi::shared::minwindef::DWORD;
#[cfg(windows)]
use winapi::um::wincon;

#[cfg(windows)]
fn get_windows_handle(
  f: &std::fs::File,
) -> Result<std::os::windows::io::RawHandle, AnyError> {
  use std::os::windows::io::AsRawHandle;
  use winapi::um::handleapi;

  let handle = f.as_raw_handle();
  if handle == handleapi::INVALID_HANDLE_VALUE {
    return Err(Error::last_os_error().into());
  } else if handle.is_null() {
    return Err(custom_error("ReferenceError", "null handle"));
  }
  Ok(handle)
}

deno_core::extension!(
  deno_tty,
  ops = [op_stdin_set_raw, op_isatty, op_console_size],
  customizer = |ext: &mut deno_core::ExtensionBuilder| {
    ext.force_op_registration();
  },
);

// ref: <https://learn.microsoft.com/en-us/windows/console/setconsolemode>
#[cfg(windows)]
const COOKED_MODE: DWORD =
  // enable line-by-line input (returns input only after CR is read)
  wincon::ENABLE_LINE_INPUT
  // enables real-time character echo to console display (requires ENABLE_LINE_INPUT)
  | wincon::ENABLE_ECHO_INPUT
  // system handles CTRL-C (with ENABLE_LINE_INPUT, also handles BS, CR, and LF) and other control keys (when using `ReadFile` or `ReadConsole`)
  | wincon::ENABLE_PROCESSED_INPUT;

#[cfg(windows)]
fn mode_raw_input_on(original_mode: DWORD) -> DWORD {
  original_mode & !COOKED_MODE | wincon::ENABLE_VIRTUAL_TERMINAL_INPUT
}

#[cfg(windows)]
fn mode_raw_input_off(original_mode: DWORD) -> DWORD {
  original_mode & !wincon::ENABLE_VIRTUAL_TERMINAL_INPUT | COOKED_MODE
}

#[op(fast)]
fn op_stdin_set_raw(
  state: &mut OpState,
  is_raw: bool,
  cbreak: bool,
) -> Result<(), AnyError> {
  let rid = 0; // stdin is always rid=0

  // From https://github.com/kkawakam/rustyline/blob/master/src/tty/windows.rs
  // and https://github.com/kkawakam/rustyline/blob/master/src/tty/unix.rs
  // and https://github.com/crossterm-rs/crossterm/blob/e35d4d2c1cc4c919e36d242e014af75f6127ab50/src/terminal/sys/windows.rs
  // Copyright (c) 2015 Katsu Kawakami & Rustyline authors. MIT license.
  // Copyright (c) 2019 Timon. MIT license.
  #[cfg(windows)]
  {
    use std::os::windows::io::AsRawHandle;
    use winapi::shared::minwindef::FALSE;
    use winapi::um::consoleapi;
    use winapi::um::handleapi;

    if cbreak {
      return Err(deno_core::error::not_supported());
    }

    StdFileResource::with_file(state, rid, move |std_file| {
      let handle = std_file.as_raw_handle();

      if handle == handleapi::INVALID_HANDLE_VALUE {
        return Err(Error::last_os_error().into());
      } else if handle.is_null() {
        return Err(custom_error("ReferenceError", "null handle"));
      }
      let mut original_mode: DWORD = 0;
      // SAFETY: winapi call
      if unsafe { consoleapi::GetConsoleMode(handle, &mut original_mode) }
        == FALSE
      {
        return Err(Error::last_os_error().into());
      }

      let new_mode = if is_raw {
        mode_raw_input_on(original_mode)
      } else {
        mode_raw_input_off(original_mode)
      };

      // SAFETY: winapi call
      if unsafe { consoleapi::SetConsoleMode(handle, new_mode) } == FALSE {
        return Err(Error::last_os_error().into());
      }

      Ok(())
    })
  }
  #[cfg(unix)]
  {
    use std::os::unix::io::AsRawFd;

    StdFileResource::with_file_and_metadata(
      state,
      rid,
      move |std_file, meta_data| {
        let raw_fd = std_file.as_raw_fd();

        if is_raw {
          let mut raw = {
            let mut meta_data = meta_data.lock();
            let maybe_tty_mode = &mut meta_data.tty.mode;
            if maybe_tty_mode.is_none() {
              // Save original mode.
              let original_mode = termios::tcgetattr(raw_fd)?;
              maybe_tty_mode.replace(original_mode);
            }
            maybe_tty_mode.clone().unwrap()
          };

          raw.input_flags &= !(termios::InputFlags::BRKINT
            | termios::InputFlags::ICRNL
            | termios::InputFlags::INPCK
            | termios::InputFlags::ISTRIP
            | termios::InputFlags::IXON);

          raw.control_flags |= termios::ControlFlags::CS8;

          raw.local_flags &= !(termios::LocalFlags::ECHO
            | termios::LocalFlags::ICANON
            | termios::LocalFlags::IEXTEN);
          if !cbreak {
            raw.local_flags &= !(termios::LocalFlags::ISIG);
          }
          raw.control_chars[termios::SpecialCharacterIndices::VMIN as usize] =
            1;
          raw.control_chars[termios::SpecialCharacterIndices::VTIME as usize] =
            0;
          termios::tcsetattr(raw_fd, termios::SetArg::TCSADRAIN, &raw)?;
        } else {
          // Try restore saved mode.
          if let Some(mode) = meta_data.lock().tty.mode.take() {
            termios::tcsetattr(raw_fd, termios::SetArg::TCSADRAIN, &mode)?;
          }
        }

        Ok(())
      },
    )
  }
}

#[op(fast)]
fn op_isatty(
  state: &mut OpState,
  rid: u32,
  out: &mut [u8],
) -> Result<(), AnyError> {
  StdFileResource::with_file(state, rid, move |std_file| {
    #[cfg(windows)]
    {
      use winapi::shared::minwindef::FALSE;
      use winapi::um::consoleapi;

      let handle = get_windows_handle(std_file)?;
      let mut test_mode: DWORD = 0;
      // If I cannot get mode out of console, it is not a console.
      // TODO(bartlomieju):
      #[allow(clippy::undocumented_unsafe_blocks)]
      {
        out[0] = unsafe {
          consoleapi::GetConsoleMode(handle, &mut test_mode) != FALSE
        } as u8;
      }
    }
    #[cfg(unix)]
    {
      use std::os::unix::io::AsRawFd;
      let raw_fd = std_file.as_raw_fd();
      // TODO(bartlomieju):
      #[allow(clippy::undocumented_unsafe_blocks)]
      {
        out[0] = unsafe { libc::isatty(raw_fd as libc::c_int) == 1 } as u8;
      }
    }
    Ok(())
  })
}

#[op(fast)]
fn op_console_size(
  state: &mut OpState,
  result: &mut [u32],
) -> Result<(), AnyError> {
  fn check_console_size(
    state: &mut OpState,
    result: &mut [u32],
    rid: u32,
  ) -> Result<(), AnyError> {
    StdFileResource::with_file(state, rid, move |std_file| {
      let size = console_size(std_file)?;
      result[0] = size.cols;
      result[1] = size.rows;
      Ok(())
    })
  }

  let mut last_result = Ok(());
  // Since stdio might be piped we try to get the size of the console for all
  // of them and return the first one that succeeds.
  for rid in [0, 1, 2] {
    last_result = check_console_size(state, result, rid);
    if last_result.is_ok() {
      return last_result;
    }
  }

  last_result
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct ConsoleSize {
  pub cols: u32,
  pub rows: u32,
}

pub fn console_size(
  std_file: &std::fs::File,
) -> Result<ConsoleSize, std::io::Error> {
  #[cfg(windows)]
  {
    use std::os::windows::io::AsRawHandle;
    let handle = std_file.as_raw_handle();

    // SAFETY: winapi calls
    unsafe {
      let mut bufinfo: winapi::um::wincon::CONSOLE_SCREEN_BUFFER_INFO =
        std::mem::zeroed();

      if winapi::um::wincon::GetConsoleScreenBufferInfo(handle, &mut bufinfo)
        == 0
      {
        return Err(Error::last_os_error());
      }
      Ok(ConsoleSize {
        cols: bufinfo.dwSize.X as u32,
        rows: bufinfo.dwSize.Y as u32,
      })
    }
  }

  #[cfg(unix)]
  {
    use std::os::unix::io::AsRawFd;

    let fd = std_file.as_raw_fd();
    // SAFETY: libc calls
    unsafe {
      let mut size: libc::winsize = std::mem::zeroed();
      if libc::ioctl(fd, libc::TIOCGWINSZ, &mut size as *mut _) != 0 {
        return Err(Error::last_os_error());
      }
      Ok(ConsoleSize {
        cols: size.ws_col as u32,
        rows: size.ws_row as u32,
      })
    }
  }
}

#[cfg(all(test, windows))]
mod tests {
  #[test]
  fn test_winos_raw_mode_transitions() {
    use crate::ops::tty::mode_raw_input_off;
    use crate::ops::tty::mode_raw_input_on;

    let known_off_modes =
      [0xf7 /* Win10/CMD */, 0x1f7 /* Win10/WinTerm */];
    let known_on_modes =
      [0x2f0 /* Win10/CMD */, 0x3f0 /* Win10/WinTerm */];

    // assert known transitions
    assert_eq!(known_on_modes[0], mode_raw_input_on(known_off_modes[0]));
    assert_eq!(known_on_modes[1], mode_raw_input_on(known_off_modes[1]));

    // assert ON-OFF round-trip is neutral
    assert_eq!(
      known_off_modes[0],
      mode_raw_input_off(mode_raw_input_on(known_off_modes[0]))
    );
    assert_eq!(
      known_off_modes[1],
      mode_raw_input_off(mode_raw_input_on(known_off_modes[1]))
    );
  }
}
