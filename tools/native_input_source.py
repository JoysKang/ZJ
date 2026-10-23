"""Select an already-enabled macOS ABC source for a benchmark, then restore it."""
from contextlib import contextmanager
import ctypes as C
import time


class InputSources:
    def __init__(self):
        self.carbon = C.CDLL('/System/Library/Frameworks/Carbon.framework/Carbon')
        self.cf = C.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
        for lib, name, args, result in (
            (self.carbon, 'TISCopyCurrentKeyboardInputSource', [], C.c_void_p),
            (self.carbon, 'TISCreateInputSourceList', [C.c_void_p, C.c_uint8], C.c_void_p),
            (self.carbon, 'TISGetInputSourceProperty', [C.c_void_p, C.c_void_p], C.c_void_p),
            (self.carbon, 'TISSelectInputSource', [C.c_void_p], C.c_int32),
            (self.cf, 'CFArrayGetCount', [C.c_void_p], C.c_long),
            (self.cf, 'CFArrayGetValueAtIndex', [C.c_void_p, C.c_long], C.c_void_p),
            (self.cf, 'CFStringGetCString', [C.c_void_p, C.c_char_p, C.c_long, C.c_uint32], C.c_uint8),
            (self.cf, 'CFRelease', [C.c_void_p], None),
        ):
            function = getattr(lib, name)
            function.argtypes, function.restype = args, result
        self.id_key = C.c_void_p.in_dll(self.carbon, 'kTISPropertyInputSourceID')

    def source_id(self, source):
        value = self.carbon.TISGetInputSourceProperty(source, self.id_key)
        buffer = C.create_string_buffer(1024)
        if not value or not self.cf.CFStringGetCString(value, buffer, len(buffer), 0x08000100):
            raise RuntimeError('Cannot read native input source ID')
        return buffer.value.decode()

    def current(self):
        source = self.carbon.TISCopyCurrentKeyboardInputSource()
        if not source:
            raise RuntimeError('Cannot get current native input source')
        try:
            return self.source_id(source)
        finally:
            self.cf.CFRelease(source)

    def select(self, source):
        wanted = self.source_id(source)
        if self.carbon.TISSelectInputSource(source) != 0:
            raise RuntimeError(f'Cannot select input source {wanted}')
        for _ in range(20):
            if self.current() == wanted:
                return
            time.sleep(0.05)
        raise RuntimeError(f'Input source did not switch to {wanted}')

    def require_abc(self):
        if self.current() != 'com.apple.keylayout.ABC':
            raise RuntimeError('Input source changed during ABC latency measurement')


@contextmanager
def abc_input_source():
    sources = InputSources()
    original = sources.carbon.TISCopyCurrentKeyboardInputSource()
    if not original:
        raise RuntimeError('Cannot retain original input source')
    enabled = None
    original_id = None
    try:
        original_id = sources.source_id(original)
        enabled = sources.carbon.TISCreateInputSourceList(None, False)
        if not enabled:
            raise RuntimeError('Cannot list enabled input sources')
        for index in range(sources.cf.CFArrayGetCount(enabled)):
            source = sources.cf.CFArrayGetValueAtIndex(enabled, index)
            if source and sources.source_id(source) == 'com.apple.keylayout.ABC':
                sources.select(source)
                sources.require_abc()
                yield sources
                return
        raise RuntimeError('Enable the ABC keyboard input source before measuring latency')
    finally:
        try:
            sources.select(original)
            if sources.current() != original_id:
                raise RuntimeError('Original input source was not restored')
        except Exception as error:
            raise RuntimeError('Failed to restore original input source') from error
        finally:
            if enabled:
                sources.cf.CFRelease(enabled)
            sources.cf.CFRelease(original)
