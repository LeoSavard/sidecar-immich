<!-- SPDX-License-Identifier: Apache-2.0
Every version of this photo's edit, newest first, drawn as a tree: the line through the current version is the
leftmost lane, and each branch — an older version edited again, or another device saving at the same time —
gets a lane of its own. Nothing here deletes anything: opening a version moves "current" to it, and editing
from there starts a new branch. -->
<script lang="ts">
	import type { HistoryNode, HistoryState } from '$lib/types';
	import { describeChanges, PICKS } from '$lib/state/edit-view';

	type Props = {
		history: HistoryState;
		compareId: string | null;
		busy?: boolean;
		onopen: (id: string) => void;
		oncompare: (id: string | null) => void;
		onpick: (id: string, section: string) => void;
		onlabel: (id: string, label: string | null) => void;
	};
	let { history, compareId, busy = false, onopen, oncompare, onpick, onlabel }: Props = $props();

	const ROW = 60;
	const LANE = 14;
	const DOT = 5;

	let selected = $state<string | null>(null);
	let renaming = $state(false);

	/** Lanes: the current version's ancestry is lane 0; a node's first child continues its lane, the rest branch. */
	const graph = $derived.by(() => {
		const nodes = history.nodes;
		const byId = new Map(nodes.map((n) => [n.id, n]));
		const order = new Map(nodes.map((n, i) => [n.id, i]));
		const children = new Map<string, HistoryNode[]>();
		for (const n of nodes) if (n.parent) children.set(n.parent, [...(children.get(n.parent) ?? []), n]);
		const current = new Set<string>();
		for (let at: HistoryNode | undefined = byId.get(history.head); at; at = at.parent ? byId.get(at.parent) : undefined) current.add(at.id);

		const lane = new Map<string, number>();
		let lanes = 0;
		const root = nodes.find((n) => !n.parent);
		const stack: [HistoryNode, number][] = root ? [[root, lanes++]] : [];
		while (stack.length) {
			const [n, l] = stack.pop()!;
			lane.set(n.id, l);
			const kids = [...(children.get(n.id) ?? [])].sort((a, b) => Number(current.has(b.id)) - Number(current.has(a.id)) || order.get(a.id)! - order.get(b.id)!);
			kids.forEach((k, i) => stack.push([k, i === 0 ? l : lanes++]));
		}

		const rows = [...nodes].reverse();
		const row = new Map(rows.map((n, i) => [n.id, i]));
		const x = (l: number) => LANE / 2 + l * LANE;
		const y = (r: number) => r * ROW + ROW / 2;
		const edges = nodes
			.filter((n) => n.parent && row.has(n.parent))
			.map((n) => {
				const [x1, y1, x2, y2] = [x(lane.get(n.id)!), y(row.get(n.id)!), x(lane.get(n.parent!)!), y(row.get(n.parent!)!)];
				const d = x1 === x2 ? `M${x1},${y1}V${y2}` : `M${x1},${y1}V${y2 - ROW / 2}C${x1},${y2 - ROW / 4} ${x2},${y2 - ROW / 4} ${x2},${y2}`;
				return { d, main: current.has(n.id) };
			});
		return { rows, lane, byId, edges, width: x(Math.max(lanes - 1, 0)) + LANE / 2, x, y };
	});

	function title(n: HistoryNode): string {
		if (n.label) return n.label;
		return { original: 'Original', imported: 'Before history', save: 'Saved', auto: 'Checkpoint' }[n.kind];
	}

	function changes(n: HistoryNode): string {
		const parent = n.parent ? graph.byId.get(n.parent) : undefined;
		if (!parent) return 'As the file came';
		const list = describeChanges(parent.edit, n.edit);
		if (list.length === 0) return 'No changes';
		return list.slice(0, 3).join(' · ') + (list.length > 3 ? ` +${list.length - 3} more` : '');
	}

	function ago(stamp: string): string {
		const s = (Date.now() - Date.parse(stamp)) / 1000;
		if (!Number.isFinite(s)) return '';
		if (s < 60) return 'just now';
		if (s < 3600) return `${Math.floor(s / 60)} min ago`;
		if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
		return new Date(stamp).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
	}

	const exportBadge = (e: HistoryNode['exports'][number]) => [e.version && `v${e.version}`, e.format?.toUpperCase()].filter(Boolean).join(' ');
	const sel = $derived(selected ? graph.byId.get(selected) : undefined);

	function rename(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		if (sel) onlabel(sel.id, input.value.trim() || null);
		renaming = false;
	}
</script>

<div class="grid gap-2">
	<div class="flex items-center justify-between gap-2 text-xs">
		<span class="text-fg-2">{history.nodes.length} {history.nodes.length === 1 ? 'version' : 'versions'}</span>
		<button type="button" class="rounded px-2 py-1 text-fg-2 hover:bg-raised disabled:opacity-40" disabled={busy || history.head === 'original'} onclick={() => onopen('original')}>Back to original</button>
	</div>

	{#if sel}
		<div class="grid gap-2 rounded bg-raised p-2 text-xs">
			<div class="flex items-center justify-between gap-2">
				{#if renaming}
					<input
						class="min-w-0 flex-1 rounded bg-panel px-2 py-1 text-fg outline-none"
						value={sel.label ?? ''}
						placeholder="Name this version"
						aria-label="Version name"
						onblur={rename}
						onkeydown={(e) => e.key === 'Enter' && (e.currentTarget as HTMLInputElement).blur()}
						{@attach (el) => el.focus()}
					/>
				{:else}
					<span class="truncate font-medium text-fg">{title(sel)}</span>
				{/if}
				<button type="button" class="shrink-0 text-fg-2" aria-label="Close" onclick={() => (selected = null)}>✕</button>
			</div>
			<div class="flex flex-wrap gap-1.5">
				<button type="button" class="rounded bg-fg px-2.5 py-1 text-ground disabled:opacity-40" disabled={busy || sel.id === history.head} onclick={() => onopen(sel.id)}>{sel.id === history.head ? 'Current' : 'Open'}</button>
				<button type="button" aria-pressed={compareId === sel.id} class="rounded px-2.5 py-1 {compareId === sel.id ? 'bg-fg text-ground' : 'bg-panel'}" onclick={() => oncompare(compareId === sel.id ? null : sel.id)}>Compare</button>
				<button type="button" class="rounded bg-panel px-2.5 py-1" onclick={() => (renaming = true)}>{sel.label ? 'Rename' : 'Name'}</button>
			</div>
			{#if sel.id !== history.head}
				<div class="flex flex-wrap items-center gap-1.5">
					<span class="text-fg-2">Use its</span>
					{#each PICKS as p (p.id)}
						<button type="button" class="rounded bg-panel px-2 py-1 disabled:opacity-40" disabled={busy} onclick={() => onpick(sel.id, p.id)}>{p.label}</button>
					{/each}
				</div>
			{/if}
			{#if compareId === sel.id}<p class="text-fg-2">Hold the photo to see this version.</p>{/if}
		</div>
	{/if}

	<div class="relative" style:height="{graph.rows.length * ROW}px">
		<svg class="pointer-events-none absolute top-0 left-0" width={graph.width} height={graph.rows.length * ROW} aria-hidden="true">
			{#each graph.edges as e, i (i)}
				<path d={e.d} fill="none" stroke="currentColor" stroke-width="1.5" class={e.main ? 'text-fg-2' : 'text-muted'} />
			{/each}
			{#each graph.rows as n, r (n.id)}
				<circle
					cx={graph.x(graph.lane.get(n.id) ?? 0)}
					cy={graph.y(r)}
					r={n.id === history.head ? DOT + 1 : DOT}
					class={n.id === history.head ? 'fill-fg' : n.kind === 'auto' ? 'fill-panel stroke-fg-2' : 'fill-fg-2'}
					stroke-width="1.5"
				/>
			{/each}
		</svg>
		{#each graph.rows as n, r (n.id)}
			<button
				type="button"
				class="absolute right-0 flex flex-col justify-center rounded pr-2 text-left {selected === n.id ? 'bg-raised' : 'hover:bg-raised/60'}"
				style:top="{r * ROW}px"
				style:height="{ROW}px"
				style:left="{graph.width + 6}px"
				aria-current={n.id === history.head ? 'true' : undefined}
				onclick={() => { selected = selected === n.id ? null : n.id; renaming = false; }}
			>
				<span class="flex min-w-0 items-center gap-1.5 text-sm">
					<span class="truncate {n.id === history.head ? 'font-medium text-fg' : 'text-fg'}">{title(n)}</span>
					{#if n.id === history.head}<span class="shrink-0 rounded bg-fg px-1 text-[10px] text-ground">Current</span>{/if}
					{#if compareId === n.id}<span class="shrink-0 rounded bg-raised px-1 text-[10px] text-fg">Comparing</span>{/if}
					{#each n.exports as e, i (i)}<span class="shrink-0 rounded border border-line px-1 text-[10px] text-fg-2">{exportBadge(e) || 'Exported'}</span>{/each}
				</span>
				<span class="truncate text-[11px] text-fg-2">{changes(n)}</span>
				<span class="truncate text-[10px] text-muted">{ago(n.created)}{n.user ? ` · ${n.user}` : ''}</span>
			</button>
		{/each}
	</div>
</div>
