<script setup lang="ts">
import { formatMoney } from '@/components/excel/ledger-detail/columns'

defineProps<{
  value: number
}>()

const emit = defineEmits<{
  (e: 'drill'): void
}>()
</script>

<template>
  <span v-if="!value" class="text-gray-300">-</span>
  <button
    v-else
    class="amount-link"
    :class="{ 'text-[#B91C1C]': value < 0 }"
    title="查看构成明细"
    @click.stop="emit('drill')"
  >
    {{ formatMoney(value) }}
  </button>
</template>

<style scoped lang="scss">
.amount-link {
  @apply tabular-nums underline-offset-2 hover:text-[#107C41] hover:underline;
}
</style>
