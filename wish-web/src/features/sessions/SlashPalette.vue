<script setup lang="ts">
import Icon from '../../ui/components/Icon.vue';
import { tr } from '../../core/i18n/tr.ts';
import type { SlashItem } from './useSlashPalette.ts';

defineProps<{ items: SlashItem[]; active: number }>();
const emit = defineEmits<{ choose: [key: string]; hover: [index: number] }>();

const groupLabel = (kind: string) => (kind === 'skill' ? tr('技能', 'Skills') : tr('命令', 'Commands'));
</script>

<template>
  <div class="slash-palette" role="listbox" :aria-label="tr('斜杠命令','Slash commands')">
    <ul class="slash-list">
      <template v-for="(item, index) in items" :key="item.key">
        <li v-if="index === 0 || items[index - 1].kind !== item.kind" class="slash-group" aria-hidden="true">{{ groupLabel(item.kind) }}</li>
        <li class="slash-option" role="option" :aria-selected="index === active" :class="{ active: index === active }"
          @mousedown.prevent="emit('choose', item.key)" @mousemove="emit('hover', index)">
          <span class="slash-icon"><Icon :name="item.icon" /></span>
          <span class="slash-text"><span class="slash-title">{{ item.title }}</span><small v-if="item.desc">{{ item.desc }}</small></span>
        </li>
      </template>
      <li v-if="!items.length" class="slash-empty">{{ tr('没有匹配的命令或技能', 'No matching command or skill') }}</li>
    </ul>
  </div>
</template>

<style scoped>
.slash-palette { position: absolute; left: 0; right: 0; bottom: calc(100% + 8px); z-index: 60; max-height: min(320px, 52dvh); overflow: auto; overscroll-behavior: contain; padding: 6px; background: var(--bg-raised); border: 1px solid var(--line-strong); border-radius: 12px; box-shadow: var(--shadow-pop); }
.slash-list { list-style: none; margin: 0; padding: 0; }
.slash-group { padding: 6px 10px 2px; font-size: 11px; font-weight: 600; letter-spacing: .04em; color: var(--fg-subtle); }
.slash-group:not(:first-child) { margin-top: 2px; border-top: 1px solid var(--line); padding-top: 8px; }
.slash-option { display: flex; align-items: center; gap: 10px; padding: 8px 10px; border-radius: 8px; cursor: pointer; }
.slash-option.active { background: var(--bg-hover); }
.slash-icon { display: grid; place-items: center; flex: none; width: 26px; height: 26px; border-radius: 7px; background: var(--bg-inset); color: var(--fg-muted); }
.slash-icon :deep(.icon) { width: 15px; height: 15px; }
.slash-option.active .slash-icon { background: var(--accent-soft); color: var(--accent); }
.slash-text { min-width: 0; flex: 1; display: flex; flex-direction: column; gap: 1px; }
.slash-title { font-size: 13.5px; font-weight: 500; color: var(--fg); }
.slash-text small { font-size: 11.5px; color: var(--fg-subtle); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.slash-empty { padding: 14px 10px; text-align: center; font-size: 12.5px; color: var(--fg-subtle); }
</style>
