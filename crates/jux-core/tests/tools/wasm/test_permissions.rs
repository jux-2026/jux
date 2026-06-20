use jux_core::{
    WasmEnvironmentCapability, WasmEnvironmentPermission, WasmFilesystemCapability,
    WasmFilesystemPermission, WasmNetworkCapability, WasmNetworkPermission,
    WasmPackageLoadingCapability, WasmPermissions, WasmStdioCapability, WasmerRuntimeCapabilities,
};

#[test]
fn wasm_permissions_convert_to_wasmer_runtime_capabilities() {
    let capabilities = WasmerRuntimeCapabilities::from(WasmPermissions {
        filesystem: WasmFilesystemPermission::HostDirectoryMapping,
        environment: WasmEnvironmentPermission::ForwardHost,
        network: WasmNetworkPermission::HttpClient,
    });

    assert_eq!(
        capabilities,
        WasmerRuntimeCapabilities {
            filesystem: WasmFilesystemCapability::MappedHostDirectory,
            environment: WasmEnvironmentCapability::ForwardHost,
            stdio: WasmStdioCapability::Buffered,
            network: WasmNetworkCapability::HttpClient,
            http_policy: None,
            package_loading: WasmPackageLoadingCapability::BuiltinWithHttpClient,
        }
    );
}
