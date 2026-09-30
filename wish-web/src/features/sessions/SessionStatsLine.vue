<script setup lang="ts">
// One quiet dsh-style counters line under the composer: turns, steps, speed,
// cumulative tokens, cache hit, and how close compaction is.
import { computed } from 'vue';
import type { SessionView } from '../../core/api/projections.ts';
import { contextGauge } from './contextUsage.ts';
import { fmtTokens } from '../../core/util/fmt.ts';
import { tr } from '../../core/i18n/tr.ts';

const props = defineProps<{ snapshot: SessionView | null }>();

const parts = computed(() => {
  const snapshot = props.snapshot;
  const stats = snapshot?.stats;
  if (!snapshot || !stats) return null;
  const tokens = stats.input_tokens + stats.output_tokens;
  const gauge = contextGauge(snapshot.context_tokens, snapshot.config?.compaction?.trigger_tokens, null);
  return [
    tr(`${stats.turns} 轮 ${stats.steps} 步`, `${stats.turns} turns ${stats.steps} steps`),
    stats.rate == null ? null : `${Math.round(stats.rate)} tok/s`,
    `${fmtTokens(tokens)} tok`,
    stats.cache_hit == null ? null : tr(`缓存命中 ${Math.round(stats.cache_hit * 100)}%`, `cache ${Math.round(stats.cache_hit * 100)}%`),
    gauge.ratio == null ? null : tr(`上下文 ${Math.round(gauge.ratio * 100)}%`, `context ${Math.round(gauge.ratio * 100)}%`),
  ].filter((part): part is string => part != null);
});
</script>

<template>
  <div v-if="parts" class="session-stats-line">{{ parts.join(' · ') }}</div>
</template>

<style scoped>
.session-stats-line {
  flex: none;
  padding: 6px 14px 10px;
  text-align: center;
  font-size: 11px;
  line-height: 1.2;
  color: var(--text-faint, rgba(127, 127, 127, 0.9));
  user-select: none;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
</style>
