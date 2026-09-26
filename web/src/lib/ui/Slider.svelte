<!-- SPDX-License-Identifier: Apache-2.0
Thumb-first slider:
- the whole row is the target; touching it never jumps the value, dragging moves it from where it is
- sliding the finger up off the row lowers the gain (full, ½, ¼) for fine adjustment
- the default is a detent: crossing it catches there, and pushing on releases it without a jump
  (iOS Safari exposes no haptics; Android also vibrates)
- press and hold for fine mode: the row spans a fifth of the range around the value, in tenths of a step;
  hold again, press Escape or touch anywhere else to leave it
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
	let root: HTMLDivElement;
	let active = $state(false);
	let flash = $state(false);
	let editing = $state(false);
	let fine = $state(false);
	/** Low end of the window fine mode shows; the window is `FINE` of the range wide. */
	let windowLo = $state(0);
	let lastX = 0;
	let raw = 0;
	let moved = false;
	let lastTapAt = 0;
	let engaged = false;
	let slack = 0;
	let held = false;
	let holdTimer: ReturnType<typeof setTimeout> | undefined;
	/** Caught in the default's detent, and how far the finger has pushed against it since. */
	let caught = false;
	let push = 0;

	/** Sideways travel that turns a touch into a slider drag rather than a scroll. */
	const ENGAGE_PX = 4;
	/** Press this long without moving to toggle fine mode. */
	const HOLD_MS = 450;
	/** Fine mode's share of the range across the row: five times the precision. */
	const FINE = 0.2;

	const range = $derived(max - min);
	const span = $derived(fine ? range * FINE : range);
	const lo = $derived(fine ? windowLo : min);
	const unit = $derived(fine ? step / 10 : step);
	const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));
	const pct = (v: number) => clamp(((v - lo) / span) * 100, 0, 100);
	/** Round to a multiple of `s`, without the float noise `0.1 * 3` would leave in a sidecar. */
	const snap = (v: number, s: number) => clamp(Number((Math.round(v / s) * s).toFixed(6)), min, max);

	function centreWindow(v: number) {
		windowLo = clamp(v - (range * FINE) / 2, min, max - range * FINE);
	}

	// A value that leaves the fine window — typed, reset, or pushed past its edge — brings the window along.
	$effect(() => {
		if (fine && (value < windowLo || value > windowLo + range * FINE)) centreWindow(value);
	});

	function setFine(on: boolean) {
		fine = on;
		if (on) centreWindow(value);
		navigator.vibrate?.(10);
	}

	// Fine mode ends when the person moves on to something else.
	$effect(() => {
		if (!fine) return;
		const away = (e: PointerEvent) => !root.contains(e.target as Node) && (fine = false);
		document.addEventListener('pointerdown', away, true);
		return () => document.removeEventListener('pointerdown', away, true);
	});

	function emit(v: number) {
		const next = snap(v, unit);
		if (next !== value) onchange(next);
	}

	function down(e: PointerEvent) {
		if (disabled || editing) return;
		row.setPointerCapture(e.pointerId);
		lastX = e.clientX;
		raw = value;
		moved = false;
		engaged = false;
		held = false;
		caught = false;
		slack = 0;
		active = true;
		clearTimeout(holdTimer);
		holdTimer = setTimeout(() => {
			if (!active || engaged) return;
			held = true;
			setFine(!fine);
		}, HOLD_MS);
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
			clearTimeout(holdTimer);
		}
		const rect = row.getBoundingClientRect();
		const above = Math.max(0, rect.top - e.clientY);
		const gain = above < 44 ? 1 : above < 88 ? 0.5 : 0.25;
		const dx = e.clientX - lastX;
		lastX = e.clientX;
		if (dx === 0) return;
		moved = true;
		const delta = (dx / rect.width) * span * gain;
		// The detent: `push` is travel spent against it. Leaving costs that travel but never jumps the value,
		// so the first step off the default is one step — not the width of the magnetic zone, as it was.
		const zone = span * 0.02;
		if (caught) {
			push += delta;
			if (Math.abs(push) <= zone) return;
			caught = false;
			raw = defaultValue + push - Math.sign(push) * zone;
		} else {
			const next = clamp(raw + delta, min, max);
			const from = raw - defaultValue;
			const to = next - defaultValue;
			if (from !== 0 && (to === 0 || Math.sign(to) !== Math.sign(from))) {
				caught = true;
				push = 0;
				raw = defaultValue;
				flash = true;
				setTimeout(() => (flash = false), 180);
				navigator.vibrate?.(8);
			} else raw = next;
		}
		emit(raw);
	}

	function up() {
		if (!active) return;
		active = false;
		clearTimeout(holdTimer);
		if (held) return;
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
		const k = e.shiftKey ? unit * 10 : unit;
		const delta = { ArrowRight: k, ArrowUp: k, ArrowLeft: -k, ArrowDown: -k }[e.key];
		if (delta !== undefined) {
			e.preventDefault();
			onchange(snap(value + delta, unit));
			oncommit?.();
		} else if (e.key === 'Home' || e.key === 'End') {
			e.preventDefault();
			onchange(e.key === 'Home' ? min : max);
			oncommit?.();
		} else if (e.key === 'Escape' && fine) {
			fine = false;
		}
	}

	function typed(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const v = Number(input.value.replace(',', '.').replace(/[^\d.+-]/g, ''));
		if (Number.isFinite(v)) {
			// Typed values keep fine precision whatever mode the slider is in.
			onchange(snap(v, step / 10));
			oncommit?.();
		}
		editing = false;
	}

	const fillLeft = $derived(bipolar ? Math.min(pct(value), pct(defaultValue)) : 0);
	const fillWidth = $derived(bipolar ? Math.abs(pct(value) - pct(defaultValue)) : pct(value));
	const showDefaultTick = $derived(bipolar && defaultValue >= lo && defaultValue <= lo + span);
</script>

<div bind:this={root} class="grid gap-0.5 select-none {disabled ? 'opacity-40' : ''}">
	<div class="flex items-baseline justify-between text-xs">
		<span id="{id}-label" class="text-fg-2">{label}{#if fine}<span class="ml-1.5 rounded bg-raised px-1 text-[10px] tracking-wide text-fg uppercase">Fine</span>{/if}</span>
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
		aria-valuetext={fine ? `${format(value)}, fine` : format(value)}
		aria-disabled={disabled}
		onpointerdown={down}
		onpointermove={move}
		onpointerup={up}
		onpointercancel={up}
		onkeydown={key}
	>
		<div class="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded-full bg-raised" style:background={fine ? undefined : trackGradient}>
			<div class="absolute inset-y-0 rounded-full bg-fg-2" style:left="{fillLeft}%" style:width="{fillWidth}%" style:opacity={trackGradient && !fine ? 0.35 : 1}></div>
			{#if showDefaultTick}<div class="absolute top-1/2 h-3 w-px -translate-y-1/2 bg-muted" style:left="{pct(defaultValue)}%"></div>{/if}
		</div>
		<div
			class="absolute top-1/2 size-7 -translate-x-1/2 -translate-y-1/2 rounded-full border border-ground bg-fg shadow transition-transform pointer-fine:size-4 {active ? 'scale-110' : ''}"
			style:left="{pct(value)}%"
		></div>
	</div>
	{#if fine}
		<div class="flex justify-between text-[10px] text-muted tabular-nums" aria-hidden="true">
			<span>{format(snap(lo, unit))}</span>
			<span>{format(snap(lo + span, unit))}</span>
		</div>
	{/if}
</div>
