#!/usr/bin/env python3
"""Regression cases for test-only refusals contaminating the shipping ledger."""
import unittest
from lib_rust_source import strip_test_modules


class TestModuleFiltering(unittest.TestCase):
    def test_keeps_shipping_code_after_nested_test_module(self):
        before = 'fn real() { UnsupportedConfig("real refusal") }\n'
        test = '''#[cfg(test)]
mod tests {
    #[test]
    fn message() {
        let fixture = EncodeError::InvalidDimensions { reason: "test-only {" };
        // An unmatched } in a comment cannot end the module either.
        /* { */ assert!(true);
    }
}
'''.rstrip('\n')
        after = '\nfn another() { UnsupportedConfig("another refusal") }\n'
        self.assertEqual(strip_test_modules(before + test + after), before + after)

    def test_ignores_cfg_text_in_strings_and_comments(self):
        src = 'fn real() { let msg = "#[cfg(test)] {"; } // #[cfg(test)] {\n'
        self.assertEqual(strip_test_modules(src), src)

    def test_test_only_import_preserves_following_shipping_function(self):
        src = '#[cfg(test)]\nuse alloc::vec::Vec;\nfn real() { refusal() }'
        self.assertEqual(strip_test_modules(src), src)

    def test_multiple_gated_modules(self):
        src = '#[cfg(all(test, feature = "std"))]\nmod a { fn a() {} }\n'
        src += '#[cfg(test)]\nmod b {}\nfn real() {}'
        self.assertEqual(strip_test_modules(src), '\n\nfn real() {}')


if __name__ == '__main__':
    unittest.main()
