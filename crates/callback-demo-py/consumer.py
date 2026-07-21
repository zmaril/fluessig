"""The python host consumer for the callback demo. Imports the built extension
module, passes a Python callable into the generated `each_tick`, and asserts the
callable was invoked from Rust with [0, 1, 2].

`each_tick(count, listener)` lowers `listener` to a `PyObject` invoked inline
under `Python::with_gil` — synchronous, so every call has landed by the time
`each_tick` returns. No event-loop drain needed (unlike the node TSFN path).
"""

import os
import sys

# run.sh drops the renamed `callback_demo.so` into a directory on this path.
sys.path.insert(0, os.environ.get("CALLBACK_MODULE_DIR", os.path.dirname(__file__)))

import callback_demo  # noqa: E402

seen = []
callback_demo.each_tick(3, lambda v: seen.append(v))

assert seen == [0, 1, 2], f"expected [0, 1, 2], got {seen}"
print("python callback fired:", seen)
