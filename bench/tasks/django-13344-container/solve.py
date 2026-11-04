"""Apply the reference patch shared with the native SWE-bench task."""
import runpy
from pathlib import Path

runpy.run_path(str(Path(__file__).resolve().parent.parent / 'swebench-django-13344' / 'solve.py'),
               run_name='__main__')
