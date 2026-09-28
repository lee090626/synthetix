"""v8.8 DAG fixture oracle and deterministic project materializer."""

from pathlib import Path
import hashlib

SHAPES = ("chain", "wide_fan_out", "shared_leaf_fan_in", "layered_shared_path", "independent_roots")
SIZES = (8, 32, 128)
MUTATIONS = (
    "no_op", "unrelated", "leaf", "root", "import_add", "import_remove",
    "import_reorder", "module_rename", "module_delete", "manifest_only",
    "valid_invalid_recovered",
)


def graph_for(shape, size, edge_cap=False):
    if size not in SIZES:
        raise ValueError(f"unsupported graph size: {size}")
    names = [f"n{i:03}" for i in range(size)]
    nodes = {name: [] for name in names}
    if edge_cap:
        if shape != "layered_shared_path":
            raise ValueError("edge-cap topology is layered_shared_path only")
        pairs = [(a, b) for a in names for b in names if a < b]
        for a, b in pairs[:3 * size]:
            nodes[a].append(b)
        roots, label = names.copy(), "edge_cap_layered_dag"
    elif shape == "chain":
        for i in range(1, size):
            nodes[names[i]].append(names[i - 1])
        roots, label = [names[-1]], shape
    elif shape == "wide_fan_out":
        nodes[names[0]] = names[1:].copy()
        roots, label = [names[0]], shape
    elif shape == "shared_leaf_fan_in":
        for name in names[1:]:
            nodes[name].append(names[0])
        roots, label = names[1:].copy(), shape
    elif shape == "layered_shared_path":
        root_a, root_b = names[:2]
        branches = names[2:size - 3]
        join, leaf, independent = names[size - 3:]
        nodes[root_a] = branches.copy()
        nodes[root_b] = branches.copy()
        for branch in branches:
            nodes[branch] = [join]
        nodes[join] = [leaf]
        roots, label = [root_a, root_b, independent], shape
    elif shape == "independent_roots":
        roots, label = names.copy(), shape
    else:
        raise ValueError(f"unknown graph shape: {shape}")
    graph = {"shape": label, "size": size, "nodes": nodes, "roots": roots}
    validate(graph)
    return graph


def validate(graph):
    nodes, roots = graph["nodes"], graph["roots"]
    if len(nodes) != graph["size"] or len(set(roots)) != len(roots):
        raise ValueError("node/root count mismatch")
    if any(root not in nodes for root in roots):
        raise ValueError("unknown root")
    if any(dep not in nodes for deps in nodes.values() for dep in deps):
        raise ValueError("unknown dependency")
    if any(len(deps) != len(set(deps)) for deps in nodes.values()):
        raise ValueError("duplicate dependency")
    if cyclic(nodes):
        raise ValueError("expected acyclic graph")
    edges = sum(map(len, nodes.values())) + len(roots)
    if edges > 4 * graph["size"]:
        raise ValueError(f"edge budget exceeded: {edges}")
    if graph["shape"] == "edge_cap_layered_dag" and edges != 4 * graph["size"]:
        raise ValueError("edge-cap graph must have exactly 4N total edges")


def cyclic(nodes):
    active, done = set(), set()

    def visit(node):
        if node in active:
            return True
        if node in done:
            return False
        active.add(node)
        if any(visit(dep) for dep in nodes[node]):
            return True
        active.remove(node)
        done.add(node)
        return False

    return any(visit(node) for node in nodes)


def record(graph):
    nodes = graph["nodes"]
    module_edges = sorted((caller, dep) for caller, deps in nodes.items() for dep in deps)
    all_edges = sorted(module_edges + [("entry", root) for root in graph["roots"]])
    return {
        "shape": graph["shape"], "size": graph["size"],
        "nodes": sorted(nodes), "edges": [list(edge) for edge in all_edges],
        "module_edges": len(module_edges), "edge_count_including_entry": len(all_edges),
        "roots": sorted(graph["roots"]), "node_count": len(nodes),
        "module_count": len(nodes) + 1, "root_count": len(graph["roots"]),
        "contract_count": len(nodes) + len(graph["roots"]),
    }


def materialize(project, graph, schema_version=1):
    validate(graph)
    project.mkdir(parents=True, exist_ok=True)
    paths = {name: project / f"{name}.ax" for name in graph["nodes"]}
    for name, deps in graph["nodes"].items():
        imports = "".join(f'import {{ {dep} }} from "./{dep}.ax";\n' for dep in deps)
        requires = "".join(f"  requires {dep}(x) == x;\n" for dep in deps)
        paths[name].write_text(
            imports + f"export contract {name}(x: u32) -> u32 {{\n{requires}  ensures result == x;\n}}\n",
            encoding="utf-8",
        )
    imports = "".join(f'import {{ {root} }} from "./{root}.ax";\n' for root in graph["roots"])
    wrappers = "".join(
        f"export contract entry_{root}(x: u32) -> u32 {{\n"
        f"  requires {root}(x) == x;\n  ensures result == x;\n}}\n"
        for root in graph["roots"]
    )
    (project / "entry.ax").write_text(imports + wrappers, encoding="utf-8")
    config = project / "synthetix.toml"
    config.write_text(
        f'[project]\nentry = "entry.ax"\ntarget = "web"\nstorage_schema_version = {schema_version}\n',
        encoding="utf-8",
    )
    return config, paths


def contract_ids(project, graph, paths):
    active = reachable_nodes(graph)
    ids = {
        name: f"{path.resolve()}::{name}"
        for name, path in paths.items()
        if name in active
    }
    entry = (project / "entry.ax").resolve()
    ids.update({f"entry_{root}": f"{entry}::entry_{root}" for root in graph["roots"]})
    return ids


def order(graph):
    result, seen = [], set()

    def visit(node):
        if node in seen:
            return
        seen.add(node)
        for dep in graph["nodes"][node]:
            visit(dep)
        result.append(node)

    for root in graph["roots"]:
        visit(root)
    result.append("entry")
    return result


def reachable_nodes(graph):
    reachable, pending = set(), list(graph["roots"])
    while pending:
        node = pending.pop()
        if node in reachable:
            continue
        reachable.add(node)
        pending.extend(graph["nodes"][node])
    return reachable


def reverse_closure(before, after, seeds):
    reverse = {}
    for graph in (before, after):
        active = reachable_nodes(graph)
        for caller, deps in graph["nodes"].items():
            if caller not in active:
                continue
            for dep in deps:
                reverse.setdefault(dep, set()).add(caller)
        for root in graph["roots"]:
            reverse.setdefault(root, set()).add(f"entry_{root}")
    closure, todo = set(seeds), list(seeds)
    while todo:
        for caller in reverse.get(todo.pop(), ()):
            if caller not in closure:
                closure.add(caller)
                todo.append(caller)
    return closure


def affected_ids(before, after, ids, seeds):
    affected = reverse_closure(before, after, seeds)
    return sorted(value for name, value in ids.items() if name in affected)


def source_hashes(project):
    return {
        str(path.relative_to(project)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(project.rglob("*"))
        if path.is_file() and ".synthetix" not in path.parts and "dist" not in path.parts
    }


def self_test():
    for size in SIZES:
        for shape in SHAPES:
            graph = graph_for(shape, size)
            assert record(graph)["node_count"] == size
            assert order(graph)[-1] == "entry"
        cap = record(graph_for("layered_shared_path", size, edge_cap=True))
        assert cap["edge_count_including_entry"] == 4 * size
        layered = record(graph_for("layered_shared_path", size))
        assert layered["edge_count_including_entry"] == 3 * size - 11
    assert record(graph_for("chain", 128))["edge_count_including_entry"] == 128
    assert record(graph_for("wide_fan_out", 128))["module_edges"] == 127
    assert record(graph_for("shared_leaf_fan_in", 128))["module_edges"] == 127

    graph = graph_for("layered_shared_path", 8)
    project = Path("/b1-oracle")
    paths = {name: project / f"{name}.ax" for name in graph["nodes"]}
    ids = contract_ids(project, graph, paths)
    independent = graph["roots"][-1]
    unrelated = affected_ids(graph, graph, ids, [independent])
    assert {name for name, value in ids.items() if value in unrelated} == {
        independent, f"entry_{independent}"
    }

    added = graph_for("layered_shared_path", 8)
    added_root = next(name for name in added["nodes"] if name not in added["roots"])
    added["roots"].append(added_root)
    added_ids = contract_ids(project, added, paths)
    added_wrappers = {f"entry_{root}" for root in added["roots"]}
    assert {
        name for name, value in added_ids.items()
        if value in affected_ids(graph, added, added_ids, sorted(added_wrappers))
    } == added_wrappers

    removed = graph_for("layered_shared_path", 8)
    removed_root = removed["roots"].pop()
    removed_ids = contract_ids(project, removed, paths)
    remaining_wrappers = {f"entry_{root}" for root in removed["roots"]}
    assert removed_root not in removed_ids
    assert f"entry_{removed_root}" not in removed_ids
    assert {
        name for name, value in removed_ids.items()
        if value in affected_ids(graph, removed, removed_ids, sorted(remaining_wrappers))
    } == remaining_wrappers
