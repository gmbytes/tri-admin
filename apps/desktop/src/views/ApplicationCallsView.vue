<script setup lang="ts">
import type { Application } from '../core/open-access'
import { useSession } from '../stores/session'
import AccessCallHistory from '../components/AccessCallHistory.vue'
defineProps<{ application: Application }>()
const session = useSession()
</script>
<template>
  <template v-if="session.can('applications.read') && session.can('applicationLogs.read')"
    ><p>
      <strong>{{ application.name }}</strong> · <code>{{ application.id }}</code>
    </p>
    <AccessCallHistory kind="application" :id="application.id" /></template
  ><el-empty v-else description="无权查看此应用调用记录" />
</template>
