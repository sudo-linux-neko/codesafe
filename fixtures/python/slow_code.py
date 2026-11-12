"""Example Python module with a known lint issue and intentionally slow code.

Known issues:
- `os` is imported but unused (ruff F401).
- `slow_sum` is an O(n) loop where the built-in `sum(range(n))` is faster.
"""

import os  # unused import


def slow_sum(n: int) -> int:
    """Intentionally slow O(n) summation."""
    total = 0
    for i in range(n):
        total += i
    return total


def fast_sum(n: int) -> int:
    return sum(range(n))


if __name__ == "__main__":
    print(slow_sum(1000))
