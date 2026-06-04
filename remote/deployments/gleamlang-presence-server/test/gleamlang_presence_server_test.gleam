//// gleeunit entry point. Test functions live in the other `*_test.gleam`
//// files in this directory; gleeunit auto-discovers any function named
//// with the `_test` suffix and runs it.

import gleeunit

pub fn main() -> Nil {
  gleeunit.main()
}
