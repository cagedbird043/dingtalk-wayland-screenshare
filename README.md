# dingtalk-wayland-screenshare-rust

这是一个用纯 Rust 实现的 Linux 版钉钉（DingTalk）在 Wayland 会话下的屏幕共享（Screenshare）拦截和注入 Hook 工具。

## 🌟 核心特色与优势

*   **纯 Rust 实现**：零 C++ 运行时依赖（不链接 `libstdc++.so`），仅依赖 Linux 核心 C 库，具有极高的系统兼容性和生命周期。
*   **无 OpenCV 依赖**：去掉了原版 C++ 方案中庞大且容易因 ABI 升级崩盘的 OpenCV 依赖，包体积缩减到不到 300KB（原版及 OpenCV 依赖达数十兆）。
*   **零运行时堆内存分配（Zero-Allocation）**：PipeWire 视频流的最新帧接收完全复用预分配的内存缓冲区，在 30 FPS 高频运行下不触发 `malloc`/`free`，彻底消除内存抖动和 CPU 垃圾回收开销。
*   **锁粒度极致优化**：视频流获取与图像缩放逻辑与全局状态锁解耦，注入线程只锁定像素双缓冲区，绝不阻塞钉钉主线程，杜绝任何微小掉帧和卡顿。
*   **无缝兼容平替**：编译产物可作为 [yatli/dingtalk-wayland-screencast](https://github.com/yatli/dingtalk-wayland-screencast) 原 C++ 版 `libdingtalk_hook.so` 的直接平替，无需修改已有的启动脚本。

## 🛠️ 工作原理

1.  **动态库劫持 (LD_PRELOAD)**：拦截 `XShmCreateImage`、`XShmAttach` 和 `shmdt` 等 X11 共享内存 API。
2.  **自动会话检测**：自适应校验调用源。如果运行在 `tblive`（钉钉会议子进程）内部，则自动激活投屏通路。
3.  **XDG Desktop Portal 握手**：通过 D-Bus 向 Portal 发起 Screencast 请求，拉起系统原生选屏窗口，并保持 D-Bus Session 生命周期与投屏同步。
4.  **PipeWire 视频流接收**：通过 PipeWire 接收所选屏幕的实时帧，并协商 `BGRx` 等原生格式。
5.  **异步像素注入**：启动 30 FPS 后台注入线程，在内存中执行高速双线性插值缩放与通道对换，直接 `memcpy` 写入钉钉的 `XImage` 共享内存空间，实现流畅投屏。

## 📦 安装指南 (Arch Linux)

本项目已收录于 Arch 用户软件仓库 (AUR)，Arch Linux 用户可以直接通过 AUR 助手一键安装：

```bash
# 使用 yay 安装
yay -S dingtalk-wayland-screenshare-rust-git

# 或使用 paru 安装
paru -S dingtalk-wayland-screenshare-rust-git
```

安装后会自动配置环境，让您的钉钉开箱即用支持 Wayland 屏幕共享。

## 🛠️ 编译与手动安装

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

## 💖 致谢与致敬

本项目的创意想法和核心实现逻辑源自以下优秀的开源项目：

*   [yatli/dingtalk-wayland-screencast](https://github.com/yatli/dingtalk-wayland-screencast) (原 C++ 版钉钉投屏 Hook 库)
*   [xuwd1/wemeet-wayland-screenshare](https://github.com/xuwd1/wemeet-wayland-screenshare) (腾讯会议投屏 Hook，也是 yatli 版的灵感来源)

感谢原作者们的创造性想法与探索，为 Linux 桌面社区在 Wayland 下共享屏幕提供了宝贵的思路！本项目采用纯 Rust 重写，旨在优化性能、缩减体积，并彻底消除 C++ 与 OpenCV 的运行时依赖。
