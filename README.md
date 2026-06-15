# dingtalk-wayland-screenshare-rust

这是一个用纯 Rust 实现的 Linux 版钉钉（DingTalk）在 Wayland 会话下的屏幕共享（Screenshare）拦截和注入 Hook 工具。

## 🌟 核心特色与优势

*   **纯 Rust 实现**：零 C++ 运行时依赖（不链接 `libstdc++.so`），仅依赖 Linux 核心 C 库，具有极高的系统兼容性和生命周期。
*   **无 OpenCV 依赖**：去掉了原版 C++ 方案中庞大且容易因 ABI 升级崩盘的 OpenCV 依赖，包体积缩减到不到 300KB（原版及 OpenCV 依赖达数十兆）。
*   **零运行时堆内存分配（Zero-Allocation）**：PipeWire 视频流的最新帧接收完全复用预分配的内存缓冲区，在 30 FPS 高频运行下不触发 `malloc`/`free`，彻底消除内存抖动和 CPU 垃圾回收开销。
*   **锁粒度极致优化**：视频流获取与图像缩放逻辑与全局状态锁解耦，注入线程只锁定像素双缓冲区，绝不阻塞钉钉主线程，杜绝任何微小掉帧和卡顿。
*   **无缝兼容平替**：编译产物可作为原 C++ 版 `libdingtalk_hook.so` 的直接替代品，无需修改任何已有的启动脚本。

## 🛠️ 工作原理

1.  **动态库劫持 (LD_PRELOAD)**：拦截 `XShmCreateImage`、`XShmAttach` 和 `shmdt` 等 X11 共享内存 API。
2.  **自动会话检测**：自适应校验调用源。如果运行在 `tblive`（钉钉会议子进程）内部，则自动激活投屏通路。
3.  **XDG Desktop Portal 握手**：通过 D-Bus 向 Portal 发起 Screencast 请求，拉起系统原生选屏窗口，并保持 D-Bus Session 生命周期与投屏同步。
4.  **PipeWire 视频流接收**：通过 PipeWire 接收所选屏幕的实时帧，并协商 `BGRx` 等原生格式。
5.  **异步像素注入**：启动 30 FPS 后台注入线程，在内存中执行高速双线性插值缩放与通道对换，直接 `memcpy` 写入钉钉的 `XImage` 共享内存空间，实现流畅投屏。

## 📦 编译指南

确保您的系统已安装 `rust`、`cargo` 以及 `pipewire` 开发库：

```bash
cargo build --release
```

编译产物位于 `target/release/libdingtalk_wayland_screenshare.so`。

## 🚀 使用与测试

通过 `LD_PRELOAD` 预加载启动钉钉：

```bash
LD_PRELOAD=/path/to/libdingtalk_wayland_screenshare.so dingtalk
```
