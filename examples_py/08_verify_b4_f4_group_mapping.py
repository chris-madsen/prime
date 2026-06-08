from __future__ import annotations

from collections import deque
from fractions import Fraction
from itertools import product
from typing import Iterable, TypeAlias


Scalar: TypeAlias = Fraction
Vector: TypeAlias = tuple[Scalar, Scalar, Scalar, Scalar]
Matrix: TypeAlias = tuple[
    tuple[Scalar, Scalar, Scalar, Scalar],
    tuple[Scalar, Scalar, Scalar, Scalar],
    tuple[Scalar, Scalar, Scalar, Scalar],
    tuple[Scalar, Scalar, Scalar, Scalar],
]
IntegerVector: TypeAlias = tuple[int, int, int, int]
IntegerMatrix: TypeAlias = tuple[
    tuple[int, int, int, int],
    tuple[int, int, int, int],
    tuple[int, int, int, int],
    tuple[int, int, int, int],
]
Permutation: TypeAlias = tuple[int, ...]


DIMENSION = 4
IDENTITY: Matrix = tuple(
    tuple(Fraction(row == column) for column in range(DIMENSION))
    for row in range(DIMENSION)
)  # type: ignore[assignment]


def multiply(left: Matrix, right: Matrix) -> Matrix:
    return tuple(
        tuple(
            sum(
                left[row][inner] * right[inner][column]
                for inner in range(DIMENSION)
            )
            for column in range(DIMENSION)
        )
        for row in range(DIMENSION)
    )  # type: ignore[return-value]


def multiply_integer(left: IntegerMatrix, right: IntegerMatrix) -> IntegerMatrix:
    return tuple(
        tuple(
            sum(
                left[row][inner] * right[inner][column]
                for inner in range(DIMENSION)
            )
            for column in range(DIMENSION)
        )
        for row in range(DIMENSION)
    )  # type: ignore[return-value]


def act(matrix: Matrix, vector: Vector) -> Vector:
    return tuple(
        sum(matrix[row][column] * vector[column] for column in range(DIMENSION))
        for row in range(DIMENSION)
    )  # type: ignore[return-value]


def transpose(matrix: Matrix) -> Matrix:
    return tuple(
        tuple(matrix[column][row] for column in range(DIMENSION))
        for row in range(DIMENSION)
    )  # type: ignore[return-value]


def power(matrix: Matrix, exponent: int) -> Matrix:
    result = IDENTITY
    factor = matrix
    remaining = exponent
    while remaining:
        if remaining & 1:
            result = multiply(result, factor)
        factor = multiply(factor, factor)
        remaining //= 2
    return result


def matrix_order(matrix: Matrix, limit: int = 200) -> int:
    current = IDENTITY
    for exponent in range(1, limit + 1):
        current = multiply(current, matrix)
        if current == IDENTITY:
            return exponent
    raise AssertionError(f"matrix order exceeds verification limit {limit}")


def reflection(root: Vector) -> Matrix:
    norm_squared = sum(coordinate * coordinate for coordinate in root)
    return tuple(
        tuple(
            Fraction(row == column)
            - 2 * root[row] * root[column] / norm_squared
            for column in range(DIMENSION)
        )
        for row in range(DIMENSION)
    )  # type: ignore[return-value]


def generate_group(generators: Iterable[Matrix]) -> frozenset[Matrix]:
    generator_tuple = tuple(generators)
    discovered = {IDENTITY}
    queue = deque([IDENTITY])
    while queue:
        current = queue.popleft()
        for generator in generator_tuple:
            candidate = multiply(current, generator)
            if candidate not in discovered:
                discovered.add(candidate)
                queue.append(candidate)
    return frozenset(discovered)


def orbit(group: Iterable[Matrix], vector: Vector) -> frozenset[Vector]:
    return frozenset(act(element, vector) for element in group)


def compose_permutations(
    left: Permutation,
    right: Permutation,
) -> Permutation:
    return tuple(left[right[index]] for index in range(len(left)))


def permutation_power(
    permutation: Permutation,
    exponent: int,
) -> Permutation:
    result = tuple(range(len(permutation)))
    factor = permutation
    remaining = exponent
    while remaining:
        if remaining & 1:
            result = compose_permutations(result, factor)
        factor = compose_permutations(factor, factor)
        remaining //= 2
    return result


def permutation_order(permutation: Permutation) -> int:
    from math import lcm

    visited = [False] * len(permutation)
    result = 1
    for start in range(len(permutation)):
        if visited[start]:
            continue
        current = start
        cycle_length = 0
        while not visited[current]:
            visited[current] = True
            current = permutation[current]
            cycle_length += 1
        result = lcm(result, cycle_length)
    return result


def root_permutation(
    matrix: Matrix,
    roots: tuple[Vector, ...],
    root_indices: dict[Vector, int],
) -> Permutation:
    return tuple(root_indices[act(matrix, root)] for root in roots)


def f4_simple_reflections() -> tuple[Matrix, Matrix, Matrix, Matrix]:
    roots: tuple[Vector, ...] = (
        (Fraction(0), Fraction(1), Fraction(-1), Fraction(0)),
        (Fraction(0), Fraction(0), Fraction(1), Fraction(-1)),
        (Fraction(0), Fraction(0), Fraction(0), Fraction(1)),
        (
            Fraction(1, 2),
            Fraction(-1, 2),
            Fraction(-1, 2),
            Fraction(-1, 2),
        ),
    )
    return tuple(reflection(root) for root in roots)  # type: ignore[return-value]


def short_f4_roots() -> frozenset[Vector]:
    coordinate_roots = {
        tuple(
            Fraction(sign if coordinate == axis else 0)
            for coordinate in range(DIMENSION)
        )
        for axis in range(DIMENSION)
        for sign in (-1, 1)
    }
    half_roots = {
        tuple(Fraction(sign, 2) for sign in signs)
        for signs in product((-1, 1), repeat=DIMENSION)
    }
    return frozenset(coordinate_roots | half_roots)  # type: ignore[arg-type]


def long_f4_roots_integer() -> frozenset[IntegerVector]:
    roots: set[IntegerVector] = set()
    for first in range(DIMENSION):
        for second in range(first + 1, DIMENSION):
            for first_sign in (-1, 1):
                for second_sign in (-1, 1):
                    vector = [0] * DIMENSION
                    vector[first] = first_sign
                    vector[second] = second_sign
                    roots.add(tuple(vector))  # type: ignore[arg-type]
    return frozenset(roots)


def dot_integer(left: IntegerVector, right: IntegerVector) -> int:
    return sum(a * b for a, b in zip(left, right, strict=True))


def matrix_from_columns(columns: tuple[IntegerVector, ...]) -> IntegerMatrix:
    return tuple(
        tuple(columns[column][row] for column in range(DIMENSION))
        for row in range(DIMENSION)
    )  # type: ignore[return-value]


def maps_short_roots_to_normalized_long_roots(
    numerator: IntegerMatrix,
    long_roots: frozenset[IntegerVector],
) -> bool:
    for signs in product((-1, 1), repeat=DIMENSION):
        doubled_image = tuple(
            sum(
                numerator[row][column] * signs[column]
                for column in range(DIMENSION)
            )
            for row in range(DIMENSION)
        )
        if any(coordinate % 2 for coordinate in doubled_image):
            return False
        image = tuple(coordinate // 2 for coordinate in doubled_image)
        if image not in long_roots:
            return False
    return True


def exact_duality_frames() -> tuple[IntegerMatrix, ...]:
    long_roots = long_f4_roots_integer()
    frames: list[IntegerMatrix] = []
    for columns in product(long_roots, repeat=DIMENSION):
        if any(
            dot_integer(columns[left], columns[right]) != 0
            for left in range(DIMENSION)
            for right in range(left + 1, DIMENSION)
        ):
            continue
        numerator = matrix_from_columns(columns)
        if maps_short_roots_to_normalized_long_roots(numerator, long_roots):
            frames.append(numerator)
    return tuple(frames)


def verify_b4_subgroup() -> None:
    rho: Matrix = (
        (Fraction(0), Fraction(1), Fraction(0), Fraction(0)),
        (Fraction(0), Fraction(0), Fraction(1), Fraction(0)),
        (Fraction(0), Fraction(0), Fraction(0), Fraction(1)),
        (Fraction(1), Fraction(0), Fraction(0), Fraction(0)),
    )
    central_inversion: Matrix = tuple(
        tuple(Fraction(-1 if row == column else 0) for column in range(DIMENSION))
        for row in range(DIMENSION)
    )  # type: ignore[assignment]
    subgroup = generate_group((rho, central_inversion))
    assert matrix_order(rho) == 4
    assert matrix_order(central_inversion) == 2
    assert multiply(rho, central_inversion) == multiply(central_inversion, rho)
    assert len(subgroup) == 8


def verify_f4_mapping() -> dict[str, int]:
    simple_reflections = f4_simple_reflections()
    group = generate_group(simple_reflections)
    assert len(group) == 1152

    coxeter = IDENTITY
    for generator in simple_reflections:
        coxeter = multiply(coxeter, generator)
    assert matrix_order(coxeter) == 12

    short_roots = short_f4_roots()
    assert len(short_roots) == 24
    ordered_roots = tuple(sorted(short_roots))
    root_indices = {root: index for index, root in enumerate(ordered_roots)}
    group_permutations = tuple(
        root_permutation(element, ordered_roots, root_indices)
        for element in group
    )
    permutation_orders = tuple(
        permutation_order(permutation)
        for permutation in group_permutations
    )

    remaining = set(short_roots)
    coxeter_orbit_sizes: list[int] = []
    while remaining:
        seed = next(iter(remaining))
        current = seed
        current_orbit: set[Vector] = set()
        while current not in current_orbit:
            current_orbit.add(current)
            current = act(coxeter, current)
        coxeter_orbit_sizes.append(len(current_orbit))
        remaining.difference_update(current_orbit)
    assert sorted(coxeter_orbit_sizes) == [12, 12]

    centralizer = tuple(
        element
        for element in group
        if multiply(element, coxeter) == multiply(coxeter, element)
    )
    assert len(centralizer) == 12
    independent_involutions = tuple(
        element
        for element in centralizer
        if matrix_order(element) == 2 and element != power(coxeter, 6)
    )
    assert not independent_involutions

    order_twelve_indices = tuple(
        index
        for index, element_order in enumerate(permutation_orders)
        if element_order == 12
    )
    assert len(order_twelve_indices) == 96
    for index in order_twelve_indices:
        element = group_permutations[index]
        own_involution = permutation_power(element, 6)
        centralizer_indices = tuple(
            other_index
            for other_index, other in enumerate(group_permutations)
            if compose_permutations(other, element)
            == compose_permutations(element, other)
        )
        assert len(centralizer_indices) == 12
        assert not any(
            permutation_orders[other_index] == 2
            and group_permutations[other_index] != own_involution
            for other_index in centralizer_indices
        )

    twisted_generators = []
    for candidate in group:
        if matrix_order(candidate) != 4:
            continue
        if power(candidate, 2) != power(coxeter, 6):
            continue
        conjugate = multiply(multiply(candidate, coxeter), transpose(candidate))
        if conjugate != power(coxeter, 7):
            continue
        subgroup = generate_group((coxeter, candidate))
        if len(subgroup) != 24:
            continue
        if len(orbit(subgroup, next(iter(short_roots)))) != 24:
            continue
        twisted_generators.append(candidate)
    assert len(twisted_generators) == 4

    duality_frames = exact_duality_frames()
    assert len(duality_frames) == 1152
    twice_coxeter: IntegerMatrix = tuple(
        tuple(int(2 * coordinate) for coordinate in row)
        for row in coxeter
    )  # type: ignore[assignment]
    dual_square_roots = tuple(
        numerator
        for numerator in duality_frames
        if multiply_integer(numerator, numerator) == twice_coxeter
    )
    assert len(dual_square_roots) == 2

    return {
        "weyl_group_order": len(group),
        "coxeter_order": matrix_order(coxeter),
        "coxeter_orbits": len(coxeter_orbit_sizes),
        "order_twelve_elements": len(order_twelve_indices),
        "centralizer_order": len(centralizer),
        "twisted_generators": len(twisted_generators),
        "duality_frames": len(duality_frames),
        "dual_square_roots": len(dual_square_roots),
    }


def main() -> int:
    verify_b4_subgroup()
    facts = verify_f4_mapping()
    print("B4 subgroup C4 x C2: OK")
    for name, value in facts.items():
        print(f"{name}: {value}")
    print("exact B4/F4 invariant verification: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
