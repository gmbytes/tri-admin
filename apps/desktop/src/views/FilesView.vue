<script setup lang="ts">
import { ref } from 'vue'
import { Upload } from '@element-plus/icons-vue'
import { useSession } from '../stores/session'
import FileListPanel from '../components/FileListPanel.vue'
import FileUploadsPanel from '../components/FileUploadsPanel.vue'
import FileCategoriesPanel from '../components/FileCategoriesPanel.vue'
const session = useSession(),
  tab = ref('files'),
  revision = ref(0),
  uploads = ref<InstanceType<typeof FileUploadsPanel> | null>(null)
function openUpload() {
  tab.value = 'uploads'
  void uploads.value?.openNew()
}
</script>
<template>
  <div class="file-workspace">
    <div class="page-heading">
      <div>
        <div class="eyebrow muted">FILE WORKSPACE</div>
        <h1>文件管理</h1>
        <p>管理文件与分类，核对上传进度并继续未完成的任务。</p>
      </div>
      <el-button v-if="session.can('files.write')" type="primary" :icon="Upload" @click="openUpload"
        >上传文件</el-button
      >
    </div>
    <el-tabs v-model="tab"
      ><el-tab-pane label="文件" name="files" /><el-tab-pane
        label="上传与续传"
        name="uploads" /><el-tab-pane label="分类管理" name="categories" /></el-tabs
    ><FileListPanel v-if="tab === 'files'" :revision="revision" />
    <div v-show="tab === 'uploads'">
      <FileUploadsPanel ref="uploads" :active="tab === 'uploads'" @published="revision++" />
    </div>
    <FileCategoriesPanel v-if="tab === 'categories'" @changed="revision++" />
  </div>
</template>
<style>
.file-filter {
  width: 210px;
}
.file-picker {
  padding: 18px;
  margin: 16px 0;
  border: 1px dashed var(--el-border-color);
  border-radius: 10px;
  background: var(--el-fill-color-light);
}
.file-picker > .el-tag,
.file-picker > .subtle {
  margin-left: 12px;
}
.file-picker p {
  margin: 12px 0 0;
  overflow-wrap: anywhere;
}
.file-source-input {
  display: none;
}
.file-uploads-table .el-button {
  margin-left: 0;
  margin-right: 10px;
}
.file-uploads-table .field-error {
  display: block;
  margin-top: 6px;
}
.file-uploads-table .el-progress {
  max-width: 220px;
}
.file-filter.el-select {
  flex: 0 0 210px;
}
.file-workspace .table-toolbar {
  flex-wrap: wrap;
  padding: 16px 0;
  min-height: 84px;
}
.file-workspace .search-input {
  width: 220px;
  flex: 0 0 220px;
}
.file-workspace .sort-select {
  width: 140px;
  flex: 0 0 140px;
}
.file-workspace .pagination-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  flex-wrap: wrap;
  padding-top: 20px;
  font-size: 13px;
}
.file-workspace .pagination-row > .subtle {
  white-space: nowrap;
}
</style>
