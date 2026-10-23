"""CPU rusage is in Mach ticks; its unit is not necessarily a nanosecond."""
import unittest

from sample_resources import cpu_seconds


class CpuTimeTests(unittest.TestCase):
    def test_mach_ticks_use_the_host_timebase(self):
        self.assertAlmostEqual(cpu_seconds(6_000_000, 125 / 3), 0.25)
        self.assertEqual(cpu_seconds(250_000_000, 1), 0.25)
        self.assertEqual(cpu_seconds(0, 125 / 3), 0)


if __name__ == "__main__":
    unittest.main()
