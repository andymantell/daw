// SPDX-License-Identifier: GPL-3.0-or-later
//! Systems the app can't read video on yet.

use std::path::Path;

use crate::{Error, Frame, Info};

fn not_yet() -> Error {
    Error::NoFramework("video isn't supported on this system yet".into())
}

pub(crate) fn probe(_path: &Path) -> Result<Info, Error> {
    Err(not_yet())
}

pub(crate) struct Reader;

impl Reader {
    pub(crate) fn open(_path: &Path, _info: &Info, _size: (u32, u32)) -> Result<Reader, Error> {
        Err(not_yet())
    }

    pub(crate) fn seek(&mut self, _seconds: f64) -> Result<(), Error> {
        Err(not_yet())
    }

    pub(crate) fn next_frame(&mut self) -> Result<Option<Frame>, Error> {
        Err(not_yet())
    }
}
