#!/usr/bin/env python3
"""Pong's own panel check (platform/examples/pong/check_panel.py), on this
board: `kg board serve -- python3 check_panel.py`."""

import os
import runpy

runpy.run_path(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "examples", "pong", "check_panel.py"), run_name="__main__")
