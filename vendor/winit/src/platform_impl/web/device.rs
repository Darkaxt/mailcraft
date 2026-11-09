// SPDX-FileCopyrightText: The winit contributors
// SPDX-License-Identifier: Apache-2.0
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub i32);

impl DeviceId {
    pub const fn dummy() -> Self {
        Self(0)
    }
}
