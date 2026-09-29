<script setup lang="ts">
import Hint from '../../ui/components/Hint.vue';
import { computed } from 'vue';
import { ChevronLeft, ChevronRight } from '@lucide/vue';
import { prefs } from '../../core/state/prefsSlice.ts';
import { tr } from '../../core/i18n/tr.ts';

// The mirror of the session-list handle: a bookmark tab on the rail's own edge,
// so the rail can be closed and reopened without reaching for the chat bar.
const open = computed(() => prefs.workspaceRail.value);
const label = computed(() => open.value ? tr('收起工作区面板', 'Collapse the workspace panel') : tr('展开工作区面板', 'Expand the workspace panel'));
</script>

<template>
  <Hint :text="label"><button type="button" class="workspace-rail-toggle" :aria-label="label"
    :aria-expanded="open" aria-controls="workspace-rail"
    @click="prefs.setWorkspaceRail(!open)">
    <ChevronRight v-if="open" :size="10" aria-hidden="true" />
    <ChevronLeft v-else :size="10" aria-hidden="true" />
  </button></Hint>
</template>
