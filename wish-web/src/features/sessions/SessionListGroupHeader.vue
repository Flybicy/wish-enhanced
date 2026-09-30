<script setup lang="ts">
import Icon from '../../ui/components/Icon.vue';
import Hint from '../../ui/components/Hint.vue';
import { tr } from '../../core/i18n/tr.ts';
import { DropdownMenuRoot, DropdownMenuTrigger, DropdownMenuPortal, DropdownMenuContent, DropdownMenuItem } from 'reka-ui';
import { usePageActivity } from '../../ui/composables/usePageActivity.ts';

// A workspace heading: the folder the sessions under it run in. It collapses,
// because a list of projects is only readable when the reader can close one.
// Real directories also carry a menu that renames or deletes the whole
// project; the synthetic groups (standalone, archived) never do.
const props = defineProps<{ cwd: string; name: string; count: number; collapsed: boolean; menu?: boolean }>();
const emit = defineEmits<{ toggle: []; rename: []; delete: [] }>();
const pageActive = usePageActivity();
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
    <DropdownMenuRoot v-if="menu" :modal="false">
      <DropdownMenuTrigger class="sl-menu-trigger ws-menu-trigger" :aria-label="`${tr('项目操作', 'Project actions')} · ${cwd}`"><Icon name="ellipsis-vertical" /></DropdownMenuTrigger>
      <DropdownMenuPortal v-if="pageActive">
        <DropdownMenuContent class="menu-pop sl-menu" align="end" :side-offset="4" :collision-padding="8">
          <DropdownMenuItem class="menu-item" @select="emit('rename')"><Icon name="pencil" />{{ tr('重命名项目', 'Rename project') }}</DropdownMenuItem>
          <DropdownMenuItem class="menu-item sl-delete" @select="emit('delete')"><Icon name="trash-2" />{{ tr('删除项目', 'Delete project') }}</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
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
.ws-head { position: relative; }
.ws-menu-trigger { margin: 2px 6px 0 0; }
.ws-head:hover .ws-menu-trigger, .ws-head:focus-within .ws-menu-trigger, .ws-menu-trigger[data-state="open"] { opacity: 1; }
</style>
