fn main() {
    // Interfaz: estilo "cupertino" para los widgets estándar (barras de desplazamiento)
    // y todos los recursos (íconos SVG) embebidos: el .exe no necesita archivos al lado.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("cupertino".into())
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("no se pudo compilar la interfaz de Slint");

    // Ícono, datos de versión y manifiesto del ejecutable.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rerun-if-changed=assets/app.manifest");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set_manifest_file("assets/app.manifest")
            .set("ProductName", "LiquidControl")
            .set("FileDescription", "LiquidControl")
            .set("InternalName", "liquidcontrol.exe")
            .set("OriginalFilename", "liquidcontrol.exe");
        resource.compile().expect("no se pudieron embeber los recursos de Windows");
    }
}
