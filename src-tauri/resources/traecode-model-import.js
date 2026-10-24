async function traecodeModelImport(operation, payload) {
  try {
    const href = String(globalThis.location?.href || "");
    const match = href.match(
      /^(vscode-file:\/\/vscode-app\/.+?\/resources\/app\/)/i,
    );
    if (!match) throw new Error("TraeCode workbench root unavailable");

    const module = await import(
      match[1] + "node_modules/@byted-icube/ai-modules-chat/dist/index.mjs"
    );
    const container =
      module.default?.Container?.instance ?? module.Container?.instance;
    if (!container?.initialized || typeof container.resolve !== "function") {
      throw new Error("TraeCode model services are not initialized");
    }
    const resolve = (description) => {
      const bindings = container.bindings;
      const keys = bindings?.keys ? [...bindings.keys()] : [];
      const key = keys.find((candidate) => candidate.description === description);
      if (!key) throw new Error("TraeCode model service unavailable");
      return container.resolve(key);
    };
    const operationResult = (result) => {
      if (result == null) return { code: 0 };
      if (typeof result === "boolean") return { code: result ? 0 : 1 };
      if (result && typeof result === "object") {
        if (result.code === 0 || result.success === true) return { code: 0 };
        if (result.status === "success" || result.status === "ok") return { code: 0 };
      }
      return result;
    };

    if (operation === "list") {
      return await resolve("IModelStorageService").getModelListMapFromCache();
    }
    const nativeModels = resolve("aiAgent.IAiNativeModelService");
    if (operation === "test") {
      let userId = "";
      try {
        userId = String(
          nativeModels._clientManagerService._credentialStore.getState()
            .userProfile?.userId || "",
        );
      } catch {}
      return operationResult(await nativeModels.testModelConnectivity({
        user_id: userId,
        device_id: "",
        agent_version: "V3",
        ide_version: "",
        access_type: 0,
        connect_check_param: {
          model_name: payload.model,
          provider: "custom_openai_compatible",
          is_custom_model_id: true,
          auth_type: 0,
          ak: payload.apiKey,
          base_url: payload.baseUrl,
        },
      }));
    }
    if (operation === "delete") {
      return operationResult(await nativeModels.updateCustomModel({
        action: "delete",
        id: String(payload.id),
      }));
    }
    if (operation === "add") {
      return operationResult(await nativeModels.addCustomModel({
        model_name: payload.model,
        provider: "custom_openai_compatible",
        ak: payload.apiKey,
        is_custom: true,
        auth_type: 0,
        base_url: payload.baseUrl,
        display_name: payload.displayName,
        config_detail: payload.configDetail,
      }));
    }
    if (operation === "refresh") {
      return operationResult(await resolve("IModelService").refreshModelStore());
    }
    throw new Error("Unsupported TraeCode model operation");
  } catch (_error) {
    return { code: 1 };
  }
}
