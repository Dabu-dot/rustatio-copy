<script>
  import Card from '$lib/components/ui/card.svelte';
  import { Target } from '@lucide/svelte';
  import StopConditionSettings from './StopConditionSettings.svelte';

  let {
    stopAtRatioEnabled,
    stopAtRatio,
    randomizeRatio,
    randomRatioRangePercent,
    effectiveStopAtRatio,
    stopAtUploadedEnabled,
    stopAtUploadedGB,
    stopAtDownloadedEnabled,
    stopAtDownloadedGB,
    stopAtSeedTimeEnabled,
    stopAtSeedTimeHours,
    idleWhenNoLeechers,
    idleWhenNoSeeders,
    postStopAction,
    startWhenLeechersAboveEnabled,
    startWhenLeechersAbove,
    startWhenSeedersAboveEnabled,
    startWhenSeedersAbove,
    cyclicEnabled,
    minActiveDurationHours,
    maxActiveDurationHours,
    minInactiveDurationHours,
    maxInactiveDurationHours,
    resetSessionCountersOnCycle,
    inactiveMode,
    completionPercent = 100,
    isRunning,
    onUpdate,
  } = $props();

  // Local state (defaults match createDefaultInstance)
  let localStopAtRatioEnabled = $state(false);
  let localStopAtRatio = $state(2.0);
  let localRandomizeRatio = $state(false);
  let localRandomRatioRangePercent = $state(10);
  let localStopAtUploadedEnabled = $state(false);
  let localStopAtUploadedGB = $state(10);
  let localStopAtDownloadedEnabled = $state(false);
  let localStopAtDownloadedGB = $state(10);
  let localStopAtSeedTimeEnabled = $state(false);
  let localStopAtSeedTimeHours = $state(24);
  let localIdleWhenNoLeechers = $state(false);
  let localIdleWhenNoSeeders = $state(false);
  let localPostStopAction = $state('idle');
  let localStartWhenLeechersAboveEnabled = $state(false);
  let localStartWhenLeechersAbove = $state(0);
  let localStartWhenSeedersAboveEnabled = $state(false);
  let localStartWhenSeedersAbove = $state(0);
  let localCyclicEnabled = $state(false);
  let localMinActiveDurationHours = $state(4);
  let localMaxActiveDurationHours = $state(4);
  let localMinInactiveDurationHours = $state(2);
  let localMaxInactiveDurationHours = $state(2);
  let localResetSessionCountersOnCycle = $state(true);
  let localInactiveMode = $state('idle');

  // Track if we're currently editing to prevent external updates from interfering
  let isEditing = $state(false);
  let editTimeout;

  // Update local state when props change (only when not actively editing)
  $effect(() => {
    if (!isEditing) {
      localStopAtRatioEnabled = stopAtRatioEnabled;
      localStopAtRatio = stopAtRatio;
      localRandomizeRatio = randomizeRatio;
      localRandomRatioRangePercent = randomRatioRangePercent;
      localStopAtUploadedEnabled = stopAtUploadedEnabled;
      localStopAtUploadedGB = stopAtUploadedGB;
      localStopAtDownloadedEnabled = stopAtDownloadedEnabled;
      localStopAtDownloadedGB = stopAtDownloadedGB;
      localStopAtSeedTimeEnabled = stopAtSeedTimeEnabled;
      localStopAtSeedTimeHours = stopAtSeedTimeHours;
      localIdleWhenNoLeechers = idleWhenNoLeechers;
      localIdleWhenNoSeeders = idleWhenNoSeeders;
      localPostStopAction = postStopAction;
      localStartWhenLeechersAboveEnabled = startWhenLeechersAboveEnabled;
      localStartWhenLeechersAbove = startWhenLeechersAbove;
      localStartWhenSeedersAboveEnabled = startWhenSeedersAboveEnabled;
      localStartWhenSeedersAbove = startWhenSeedersAbove;
      localCyclicEnabled = cyclicEnabled;
      localMinActiveDurationHours = minActiveDurationHours;
      localMaxActiveDurationHours = maxActiveDurationHours;
      localMinInactiveDurationHours = minInactiveDurationHours;
      localMaxInactiveDurationHours = maxInactiveDurationHours;
      localResetSessionCountersOnCycle = resetSessionCountersOnCycle;
      localInactiveMode = inactiveMode;
    }
  });

  function updateValue(key, value) {
    isEditing = true;
    clearTimeout(editTimeout);

    if (onUpdate) {
      onUpdate({ [key]: value });
    }

    editTimeout = setTimeout(() => {
      isEditing = false;
    }, 100);
  }

  // Count active conditions
  let activeCount = $derived(
    [
      localStopAtRatioEnabled,
      localStopAtUploadedEnabled,
      localStopAtDownloadedEnabled,
      localStopAtSeedTimeEnabled,
      localIdleWhenNoLeechers,
      localIdleWhenNoSeeders,
      localStartWhenLeechersAboveEnabled,
      localStartWhenSeedersAboveEnabled,
      localCyclicEnabled,
    ].filter(Boolean).length
  );
</script>

<Card class="p-3">
  <div class="flex items-center justify-between mb-3">
    <h2 class="text-primary text-lg font-semibold flex items-center gap-2">
      <Target size={20} /> Stop Conditions
    </h2>
    {#if activeCount > 0}
      <span class="text-xs bg-primary/10 text-primary px-2 py-1 rounded-full font-medium">
        {activeCount} active
      </span>
    {/if}
  </div>

  <StopConditionSettings
    bind:stopAtRatioEnabled={localStopAtRatioEnabled}
    bind:stopAtRatio={localStopAtRatio}
    bind:randomizeRatio={localRandomizeRatio}
    bind:randomRatioRangePercent={localRandomRatioRangePercent}
    {effectiveStopAtRatio}
    bind:stopAtUploadedEnabled={localStopAtUploadedEnabled}
    bind:stopAtUploadedGB={localStopAtUploadedGB}
    bind:stopAtDownloadedEnabled={localStopAtDownloadedEnabled}
    bind:stopAtDownloadedGB={localStopAtDownloadedGB}
    bind:stopAtSeedTimeEnabled={localStopAtSeedTimeEnabled}
    bind:stopAtSeedTimeHours={localStopAtSeedTimeHours}
    bind:idleWhenNoLeechers={localIdleWhenNoLeechers}
    bind:idleWhenNoSeeders={localIdleWhenNoSeeders}
    bind:postStopAction={localPostStopAction}
    bind:startWhenLeechersAboveEnabled={localStartWhenLeechersAboveEnabled}
    bind:startWhenLeechersAbove={localStartWhenLeechersAbove}
    bind:startWhenSeedersAboveEnabled={localStartWhenSeedersAboveEnabled}
    bind:startWhenSeedersAbove={localStartWhenSeedersAbove}
    bind:cyclicEnabled={localCyclicEnabled}
    bind:minActiveDurationHours={localMinActiveDurationHours}
    bind:maxActiveDurationHours={localMaxActiveDurationHours}
    bind:minInactiveDurationHours={localMinInactiveDurationHours}
    bind:maxInactiveDurationHours={localMaxInactiveDurationHours}
    bind:resetSessionCountersOnCycle={localResetSessionCountersOnCycle}
    bind:inactiveMode={localInactiveMode}
    {completionPercent}
    disabled={isRunning}
    onchange={updates => {
      for (const [key, value] of Object.entries(updates)) updateValue(key, value);
    }}
  />
</Card>
