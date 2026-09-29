<script setup lang="ts">
import Icon from '../../ui/components/Icon.vue';
import Hint from '../../ui/components/Hint.vue';
import { tr } from '../../core/i18n/tr.ts';

// A workspace heading: the folder the sessions under it run in. It collapses,
// because a list of projects is only readable when the reader can close one.
const props = defineProps<{ cwd: string; name: string; count: number; collapsed: boolean }>();
const emit = defineEmits<{ toggle: [] }>();
</script>

<template>
  <div class="ws-head" :class="{ loose: !cwd }">
    <Hint :text="cwd || tr('未指定工作目录', 'No working directory')">
      <button type="button" class="ws-head-btn" :aria-expanded="!collapsed" :title="cwd || undefined" @click="emit('toggle')">
        <Icon v-if="cwd" :name="collapsed ? 'chevron-right' : 'chevron-down'" class="ws-caret" />
        <Icon :name="cwd ? 'folder' : 'circle-dot'" class="ws-mark" />
        <span class="ws-name">{{ cwd ? name : tr('独立会话', 'Standalone') }}</span>
        <span class="ws-count">{{ count }}</span>
      </button>
    </Hint>
  </div>
</template>

<style scoped>
.ws-head { display: flex; align-items: center; }
.ws-head-btn {
  display: flex; align-items: center; gap: 6px; width: 100%; min-width: 0; cursor: pointer;
  padding: 5px 8px; margin: 2px 0 1px; border: 0; border-radius: 8px; background: none;
  color: var(--fg-subtle); font: 600 11.5px/1.4 var(--font); letter-spacing: .03em;
}
@media (hover: hover) { .ws-head-btn:hover { background: var(--bg-hover); color: var(--fg); } }
.ws-caret { width: 12px; height: 12px; flex: none; color: var(--fg-faint); }
.ws-mark { width: 13px; height: 13px; flex: none; color: var(--accent); }
.ws-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ws-count { margin-left: auto; font-size: 10.5px; font-weight: 500; color: var(--fg-faint); font-family: var(--mono); }
.ws-head.loose .ws-mark { color: var(--fg-faint); }
</style>
