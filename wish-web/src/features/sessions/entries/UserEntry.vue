<script setup lang="ts">
import { userBubbles } from '../../../core/api/userContent.ts';
import { computed, ref } from 'vue';
import UserBubble from './UserBubble.vue';
import Icon from '../../../ui/components/Icon.vue';
import { tr } from '../../../core/i18n/tr.ts';
import { useTurnActions } from '../useTurnActions.ts';
const props = defineProps<{ item: any }>();
const bubbles = computed(() => userBubbles(props.item.entry?.payload?.content));
const groups = computed(() => {
  const result: { files?: any[]; block?: any }[] = [];
  for (const block of bubbles.value) {
    if (block.type === 'file') {
      const last = result.at(-1);
      if (last?.files) last.files.push(block);
      else result.push({ files: [block] });
    } else result.push({ block });
  }
  return result;
});

// A committed user turn carries a numeric EntryId; queued/optimistic ones do not,
// and the rewind/branch affordance only makes sense on the settled ones.
const entryId = computed<number | null>(() => {
  const id = props.item.entry?.id;
  return typeof id === 'number' ? id : null;
});
const { canAct, rewind, branch, regenerate, edit } = useTurnActions();
// Rewind drops later turns from the live thread, so it arms on the first click
// and commits on the second; it disarms itself after a few seconds.
const armed = ref(false);
let disarm: ReturnType<typeof setTimeout> | undefined;
function onRewind() {
  const id = entryId.value;
  if (id == null) return;
  if (!armed.value) {
    armed.value = true;
    clearTimeout(disarm);
    disarm = setTimeout(() => (armed.value = false), 3200);
    return;
  }
  clearTimeout(disarm);
  armed.value = false;
  void rewind(id);
}
function onBranch() {
  const id = entryId.value;
  if (id != null) void branch(id);
}
function onRegenerate() {
  if (entryId.value != null) void regenerate(props.item.entry);
}
function onEdit() {
  if (entryId.value != null) void edit(props.item.entry);
}
</script>
<template>
  <div class="user-message-parts">
    <template v-for="(group, index) in groups" :key="index">
      <div v-if="group.files" class="file-group">
        <UserBubble v-for="(file, fileIndex) in group.files" :key="fileIndex" :block="file" />
      </div>
      <UserBubble v-else :block="group.block" />
    </template>
    <div v-if="entryId != null" class="turn-actions" :class="{ disabled: !canAct }">
      <button type="button" class="turn-action" :class="{ armed }" :disabled="!canAct"
        :title="tr('回档到此处', 'Rewind to here')" @click="onRewind">
        <Icon name="undo-2" />
        <span>{{ armed ? tr('确认回档', 'Confirm rewind') : tr('回档', 'Rewind') }}</span>
      </button>
      <button type="button" class="turn-action" :disabled="!canAct"
        :title="tr('从此处分支出新会话', 'Branch a new session from here')" @click="onBranch">
        <Icon name="git-branch" />
        <span>{{ tr('分支', 'Branch') }}</span>
      </button>
      <button type="button" class="turn-action" :disabled="!canAct"
        :title="tr('丢弃回复并重新发送这条消息', 'Drop the reply and resend this message')" @click="onRegenerate">
        <Icon name="refresh-cw" />
        <span>{{ tr('重新回复', 'Regenerate') }}</span>
      </button>
      <button type="button" class="turn-action" :disabled="!canAct"
        :title="tr('回到输入框修改这条消息', 'Back to the composer to edit this message')" @click="onEdit">
        <Icon name="pencil" />
        <span>{{ tr('修改', 'Edit') }}</span>
      </button>
    </div>
  </div>
</template>
<style scoped>
.user-message-parts { display:flex; flex-direction:column; gap:8px; }
.file-group { display:flex; justify-content:flex-end; flex-wrap:wrap; gap:8px; min-width:0; }
.file-group :deep(.message-context) { flex:0 1 auto; width:auto; max-width:100%; min-width:0; }
.file-group :deep(.entry.user .bubble) { width:auto; max-width:100%; }
.file-group :deep(.message-file) { width:max-content; min-width:min(180px,calc(100vw - 48px)); max-width:min(340px,100%); box-sizing:border-box; }
/* The rewind/branch bar rides under the bubble, right-aligned to match the turn.
   It stays out of the way until the turn is hovered or focused within. */
.turn-actions {
  display:flex; justify-content:flex-end; gap:4px; margin-top:2px;
  opacity:0; transition:opacity var(--dur-fast) var(--ease-out);
}
.user-message-parts:hover .turn-actions,
.turn-actions:focus-within { opacity:1; }
.turn-actions.disabled { opacity:0 !important; pointer-events:none; }
.turn-action {
  display:inline-flex; align-items:center; gap:5px; cursor:pointer;
  padding:3px 9px; border:1px solid transparent; border-radius:999px;
  background:transparent; color:var(--fg-faint); font:500 11px/1 var(--font);
  transition:background var(--dur-fast),color var(--dur-fast),border-color var(--dur-fast);
}
.turn-action .icon { width:13px; height:13px; }
@media (hover:hover) { .turn-action:hover { background:var(--bg-hover); color:var(--fg-muted); border-color:var(--line); } }
.turn-action.armed { color:var(--accent); border-color:var(--accent); background:color-mix(in srgb, var(--accent) 8%, transparent); }
</style>
