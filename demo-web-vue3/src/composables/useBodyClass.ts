import { onBeforeUnmount, onMounted } from 'vue'

/**
 * 页面存在期间给 <body> 加一个 class。
 * 对话框、下拉菜单等弹层挂在 body 下（append-to-body / teleport），页面样式管不到它们，
 * 借 body 上的 class 让这些弹层也套用该页面的主题（见 styles/excel.scss）
 */
export function useBodyClass(name: string) {
  onMounted(() => document.body.classList.add(name))
  onBeforeUnmount(() => document.body.classList.remove(name))
}
