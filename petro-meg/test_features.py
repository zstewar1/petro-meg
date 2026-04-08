#!/usr/bin/env python3

import subprocess
import itertools
import sys
from collections.abc import Iterable

ALL_FEATURES = (
        "v1",
        "v2",
        "v3",
        "dynamic_version",
        "reader",
        "writer",
        "default",
)


def powerset(iterable, minlen=None, maxlen=None):
    """powerset([1,2,3]) --> () (1,) (2,) (3,) (1,2) (1,3) (2,3) (1,2,3)

    limit sets the maximum set length. A negative number is relative to the length of the
    input.
    """
    s = list(iterable)
    if minlen is None:
        minlen = 0
    if maxlen is None:
        maxlen = len(s)
    else:
        if maxlen < 0:
            maxlen = len(s) + maxlen
        maxlen = min(maxlen, len(s))
    combinations_per_length = (itertools.combinations(s, set_len) for set_len in range(minlen, maxlen + 1))
    return itertools.chain.from_iterable(combinations_per_length)


def features_compiles(features):
    """Return true if cargo build works with the given feature set."""
    try:
        subprocess.check_call(
            ['cargo', 'build', '-p', 'petro-meg', '--lib', '--no-default-features', '--features', features],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except subprocess.CalledProcessError:
        return False
    return True


def run_features_test(all_features):
    all_subsets = set()
    sometimes_passing = set()
    for features in powerset(all_features):
        all_subsets.add(frozenset(features))
        features_str = ','.join(features)
        if features_compiles(features_str):
            for subset in powerset(features):
                sometimes_passing.add(frozenset(subset))
        else:
            print('Feature flags', features_str, 'fails.')
    always_failing = all_subsets - sometimes_passing
    # Remove any failing combinations which are just supersets of other failing
    # combinations.
    always_failing = {
        features for features in always_failing
        # If any subset of features is also in always_failing, skip this entry, because
        # it's redundant.
        if all(frozenset(subset) not in always_failing for subset in powerset(features, minlen=1, maxlen=-1))
    }

    if always_failing:
        elems = sorted(','.join(sorted(features)) for features in always_failing)
        print('These combinations of features always fail:', ', '.join(repr(elem) for elem in elems))


def main():
    try:
        run_features_test(ALL_FEATURES)
    except KeyboardInterrupt:
        print("Interrupted", file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
