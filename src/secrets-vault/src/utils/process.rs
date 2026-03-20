/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub fn interrupt() {
    #[cfg(target_os = "windows")]
    std::process::exit(2);
    #[cfg(not(target_os = "windows"))]
    {
        let pid = std::process::id() as libc::pid_t;
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
    }
}
