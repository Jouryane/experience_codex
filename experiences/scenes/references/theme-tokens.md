# 暗色主题 Token 约定

## 目标与约束
页面改造为暗色主题，保留既有组件 API。

## 步骤与子步骤
- 在 :root 定义 token：--bg / --bg-panel / --text / --accent
- 组件样式只引用 token，不写死颜色
- 交互态使用 --accent 提升对比度
- 截图验证对比度

## 最终产物与验证
- theme.css 含 --bg、--text、--accent 三个 token
- 验证：截图对比
