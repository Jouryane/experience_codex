# 暗色主题前端改造

## 目标与约束
把首页改成暗色主题，保留现有组件 API。

## 使用的工具与插件
- 文件编辑工具
- 终端工具
- 插件：ui-kit 2.1

## 步骤与子步骤
1. 搭建组件目录
   - 创建 theme provider
2. 安装依赖 ui-kit
3. 重写 theme tokens
   - 调整对比度
4. 截图验证

## 失败与修正
- 初次对比度不足 → 调整 token

## 最终产物与验证
- src/theme.css
- git diff --stat: 4 files changed
- 验证：截图对比

## 可复用模板与参数位
- 参数化主题色、组件路径
