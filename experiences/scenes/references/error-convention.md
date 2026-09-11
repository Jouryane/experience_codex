# 错误处理约定

## 目标与约束
所有网络/IO 失败必须显式包装，禁止静默吞错。

## 步骤与子步骤
- 失败统一包装为 AppError（code + message）
- 网络错误重试一次（retry once），仍失败则抛出
- 日志中保留原始错误信息

## 最终产物与验证
- helper 文件中出现 AppError 与 retry 分支
