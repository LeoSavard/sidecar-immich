# SPDX-License-Identifier: Apache-2.0
"""Rename an ONNX graph's initializers by where the graph first uses them.

tf2onnx names the constants it folds from a counter whose order changes from run to run, so the same model
converts to different files. burn-onnx derives its generated code from those names, and the weights it
writes are keyed by them, so code and weights from two conversions don't fit together. Renamed by first
use, every conversion of the same model is byte-identical.

    python canonical_names.py in.onnx out.onnx
"""
import sys

import onnx

model = onnx.load(sys.argv[1])
graph = model.graph
initializers = {i.name for i in graph.initializer}
order = []
for node in graph.node:
    for name in node.input:
        if name in initializers and name not in order:
            order.append(name)
order += sorted(initializers - set(order))
names = {old: f"weight_{i:04d}" for i, old in enumerate(order)}
for node in graph.node:
    node.input[:] = [names.get(n, n) for n in node.input]
for init in graph.initializer:
    init.name = names[init.name]
graph.initializer.sort(key=lambda i: i.name)
for value in graph.value_info:
    value.name = names.get(value.name, value.name)
onnx.save(model, sys.argv[2])
