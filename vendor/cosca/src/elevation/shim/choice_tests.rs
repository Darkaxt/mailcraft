// SPDX-FileCopyrightText: Anna Zhukova
// SPDX-License-Identifier: Apache-2.0
use super::ShimChoice;

#[skuld::test]
fn the_default_choice_is_direct() {
    assert_eq!(ShimChoice::default(), ShimChoice::Direct);
}
