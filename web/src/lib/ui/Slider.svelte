<!-- SPDX-License-Identifier: Apache-2.0
Thumb-first slider:
- the whole row is the target; touching it never jumps the value, dragging moves it from where it is
- sliding the finger up off the row lowers the gain (full, ½, ¼) for fine adjustment
- a magnetic zone of ±2 % holds the default (iOS Safari exposes no haptics; Android also vibrates)
- double-tap resets; tap the value to type one; arrow keys step, Shift ×10 -->
<script lang="ts">
	type Props = {
		id: string;
		label: string;
		value: number;
		min: number;
		max: number;
		step?: number;
		defaultValue?: number;
		bipolar?: boolean;
		trackGradient?: string;
		disabled?: boolean;
		format?: (v: number) => string;
		onchange: (v: number) => void;
		oncommit?: () => void;
	};
	let { id, label, value, min, max, step = 1, defaultValue = min, bipolar = false, trackGradient, disabled = false, format = (v) => String(v), onchange, oncommit }: Props = $props();

	let row: HTMLDivElement;
	let active = $state(false);
	let flash = $state(false);
	let editing = $state(false);
	let lastX = 0;
	let raw = 0;
	let moved = false;
	let lastTapAt = 0;
	let engaged = false;
	let slack = 0;

	/** Sideways travel that turns a touch into a slider drag rather than a scroll. */
	const ENGAGE_PX = 4;

	const range = $derived(max - min);
	const pct = (v: number) => ((v - min) / range) * 100;
	const snapStep = (v: number) => Math.min(max, Math.max(min, Math.round(v / step) * step));

	function emit(v: number) {
		const next = Math.abs(v - defaultValue) < range * 0.02 ? defaultValue : snapStep(v);
		if (next === defaultValue && value !== defaultValue) {
			flash = true;
			setTimeout(() => (flash = false), 180);
			navigator.vibrate?.(8);
		}
		if (next !== value) onchange(next);
	}

	function down(e: PointerEvent) {
		if (disabled || editing) return;
		row.setPointerCapture(e.pointerId);
		lastX = e.clientX;
		raw = value;
		moved = false;
		engaged = false;
		slack = 0;
		active = true;
	}

	function move(e: PointerEvent) {
		if (!active) return;
		// The row allows vertical panning, so a drag only counts as a slider drag once it has travelled far
		// enough sideways; until then the browser can still take the gesture and scroll the panel.
		if (!engaged) {
			slack += e.clientX - lastX;
			lastX = e.clientX;
			if (Math.abs(slack) < ENGAGE_PX) return;
			engaged = true;
		}
		const rect = row.getBoundingClientRect();
		const above = Math.max(0, rect.top - e.clientY);
		const gain = above < 44 ? 1 : above < 88 ? 0.5 : 0.25;
		const dx = e.clientX - lastX;
		lastX = e.clientX;
		if (Math.abs(dx) > 0) moved = true;
		raw = Math.min(max, Math.max(min, raw + (dx / rect.width) * range * gain));
		emit(raw);
	}

	function up() {
		if (!active) return;
		active = false;
		const now = performance.now();
		if (!moved) {
			if (now - lastTapAt < 320) {
				onchange(defaultValue);
				oncommit?.();
				lastTapAt = 0;
				return;
			}
			lastTapAt = now;
			return;
		}
		oncommit?.();
	}

	function key(e: KeyboardEvent) {
		const k = e.shiftKey ? step * 10 : step;
		const delta = { ArrowRight: k, ArrowUp: k, ArrowLeft: -k, ArrowDown: -k }[e.key];
		if (delta !== undefined) {
			e.preventDefault();
			onchange(snapStep(value + delta));
			oncommit?.();
		} else if (e.key === 'Home' || e.key === 'End') {
			e.preventDefault();
			onchange(e.key === 'Home' ? min : max);
			oncommit?.();
		}
	}

	function typed(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const v = Number(input.value.replace(',', '.').replace(/[^\d.+-]/g, ''));
		if (Number.isFinite(v)) {
			onchange(snapStep(v));
			oncommit?.();
		}
		editing = false;
	}

	const fillLeft = $derived(bipolar ? Math.min(pct(value), pct(defaultValue)) : 0);
	const fillWidth = $derived(bipolar ? Math.abs(pct(value) - pct(defaultValue)) : pct(value));
</script>

<div class="grid gap-0.5 select-none {disabled ? 'opacity-40' : ''}">
	<div class="flex items-baseline justify-between text-xs">
		<span id="{id}-label" class="text-fg-2">{label}</span>
		{#if editing}
			<input
				class="w-20 rounded bg-raised px-1 text-right text-fg tabular-nums outline-none"
				value={value}
				inputmode="decimal"
				aria-label="{label} value"
				onblur={typed}
				onkeydown={(e) => e.key === 'Enter' && (e.currentTarget as HTMLInputElement).blur()}
				{@attach (el) => el.focus()}
			/>
		{:else}
			<button type="button" class="text-fg tabular-nums transition-colors {flash ? 'text-white underline' : ''}" disabled={disabled} onclick={() => (editing = true)}>{format(value)}</button>
		{/if}
	</div>
	<div
		bind:this={row}
		{id}
		class="relative h-11 cursor-ew-resize touch-pan-y outline-none pointer-fine:h-8"
		role="slider"
		tabindex={disabled ? -1 : 0}
		aria-labelledby="{id}-label"
		aria-valuemin={min}
		aria-valuemax={max}
		aria-valuenow={value}
		aria-valuetext={format(value)}
		aria-disabled={disabled}
		onpointerdown={down}
		onpointermove={move}
		onpointerup={up}
		onpointercancel={up}
		onkeydown={key}
	>
		<div class="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded-full bg-raised" style:background={trackGradient}>
			<div class="absolute inset-y-0 rounded-full bg-fg-2" style:left="{fillLeft}%" style:width="{fillWidth}%" style:opacity={trackGradient ? 0.35 : 1}></div>
			{#if bipolar}<div class="absolute top-1/2 h-3 w-px -translate-y-1/2 bg-muted" style:left="{pct(defaultValue)}%"></div>{/if}
		</div>
		<div
			class="absolute top-1/2 size-7 -translate-x-1/2 -translate-y-1/2 rounded-full border border-ground bg-fg shadow transition-transform pointer-fine:size-4 {active ? 'scale-110' : ''}"
			style:left="{pct(value)}%"
		></div>
	</div>
</div>
