"""Compare exact Vitest cases across a fix and an untouched baseline."""

from pathlib import Path


class IncompleteEvidence(ValueError):
    """A report cannot prove which tests ran and passed or failed."""


def outcomes(report, web, expected_files):
    """Return case outcomes keyed by checkout-relative file and full test name."""
    if not isinstance(report, dict) or not isinstance(report.get('testResults'), list):
        raise IncompleteEvidence('missing Vitest suite results')
    web = Path(web).resolve()
    expected = set(expected_files)
    files = set()
    cases = {}
    for suite in report['testResults']:
        if not isinstance(suite, dict) or not isinstance(suite.get('name'), str):
            raise IncompleteEvidence('unnamed Vitest suite')
        try:
            path = Path(suite['name']).resolve().relative_to(web).as_posix()
        except (OSError, ValueError) as error:
            raise IncompleteEvidence('Vitest suite outside checkout') from error
        if path not in expected or path in files:
            raise IncompleteEvidence('unexpected or repeated Vitest suite')
        files.add(path)
        rows = suite.get('assertionResults')
        if not isinstance(rows, list) or not rows:
            raise IncompleteEvidence('Vitest suite ran no cases')
        for row in rows:
            if not isinstance(row, dict) or not isinstance(row.get('fullName'), str) or not row['fullName']:
                raise IncompleteEvidence('unnamed Vitest case')
            if row.get('status') not in ('passed', 'failed'):
                raise IncompleteEvidence('skipped or unknown Vitest case')
            key = (path, row['fullName'])
            if key in cases:
                raise IncompleteEvidence('ambiguous Vitest case')
            cases[key] = row['status']
    if files != expected or not cases:
        raise IncompleteEvidence('not every selected test file ran')
    counts = (report.get('numTotalTests'), report.get('numPassedTests'),
              report.get('numFailedTests'), report.get('numPendingTests'))
    actual = (len(cases), sum(s == 'passed' for s in cases.values()),
              sum(s == 'failed' for s in cases.values()), 0)
    if any(type(count) is not int for count in counts) or counts != actual:
        raise IncompleteEvidence('Vitest totals do not match case results')
    return cases


def distinguishes(fixed, original_with_changes, pristine):
    """Whether a changed test passed on the fix and newly failed on the base."""
    if not fixed or set(fixed) != set(original_with_changes):
        raise IncompleteEvidence('fixed and baseline test inventories differ')
    if not set(pristine).issubset(fixed):
        raise IncompleteEvidence('original test cases are missing or renamed in the changed files')
    if any(status != 'passed' for status in fixed.values()):
        raise IncompleteEvidence('a changed test did not pass on the fix')
    if any(status != 'passed' for status in pristine.values()):
        raise IncompleteEvidence('the original test suite did not pass on the base')
    return any(status == 'failed' and pristine.get(key) in (None, 'passed')
               for key, status in original_with_changes.items())
