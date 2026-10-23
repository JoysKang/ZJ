"""Input-source cleanup tests; never call macOS or change the user's input source."""
import sys
from pathlib import Path
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_input_source as native


class FakeSources:
    def __init__(self):
        self.carbon = self.cf = self
        self.active = 'original'
        self.released = []
        self.fail_restore = False
        self.missing_abc = False

    def TISCopyCurrentKeyboardInputSource(self): return 'original'
    def TISCreateInputSourceList(self, *_): return 'list'
    def CFArrayGetCount(self, _): return 0 if self.missing_abc else 1
    def CFArrayGetValueAtIndex(self, *_): return 'abc'
    def CFRelease(self, ref): self.released.append(ref)
    def source_id(self, ref): return 'com.apple.keylayout.ABC' if ref == 'abc' else ref
    def current(self): return self.source_id(self.active)

    def select(self, ref):
        if ref == 'original' and self.fail_restore:
            raise RuntimeError('restore failed')
        self.active = ref

    def require_abc(self):
        if self.active != 'abc':
            raise RuntimeError('source changed')


class InputSourceTests(unittest.TestCase):
    def test_restores_on_success_and_body_failure(self):
        for fail in (False, True):
            fake = FakeSources()
            with self.subTest(fail=fail), patch.object(native, 'InputSources', return_value=fake):
                try:
                    with native.abc_input_source() as source:
                        self.assertEqual(source.current(), 'com.apple.keylayout.ABC')
                        if fail:
                            raise ValueError('input failed')
                except ValueError:
                    self.assertTrue(fail)
                self.assertEqual(fake.active, 'original')
                self.assertEqual(fake.released, ['list', 'original'])

    def test_missing_abc_restores_and_fails(self):
        fake = FakeSources(); fake.missing_abc = True
        with patch.object(native, 'InputSources', return_value=fake):
            with self.assertRaisesRegex(RuntimeError, 'Enable the ABC'):
                with native.abc_input_source():
                    self.fail('must not inject keys')
        self.assertEqual(fake.active, 'original')
        self.assertEqual(fake.released, ['list', 'original'])

    def test_restore_failure_is_explicit_and_releases_handles(self):
        fake = FakeSources()
        with patch.object(native, 'InputSources', return_value=fake):
            with self.assertRaisesRegex(RuntimeError, 'Failed to restore'):
                with native.abc_input_source():
                    fake.fail_restore = True
        self.assertEqual(fake.released, ['list', 'original'])

    def test_source_change_is_rejected_but_original_is_restored(self):
        fake = FakeSources()
        with patch.object(native, 'InputSources', return_value=fake):
            with self.assertRaisesRegex(RuntimeError, 'source changed'):
                with native.abc_input_source() as source:
                    fake.active = 'ime'
                    source.require_abc()
        self.assertEqual(fake.active, 'original')


if __name__ == '__main__':
    unittest.main()
