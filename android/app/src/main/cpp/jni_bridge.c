#include <jni.h>
#include <stddef.h>

extern char *rap2p_android_invoke_json(const char *req_ptr);
extern void rap2p_android_free_string(char *ptr);

JNIEXPORT jstring JNICALL
Java_io_github_niunmango_rustaudiop2p_RustAudioBridge_invokeJson(
    JNIEnv *env,
    jobject thiz,
    jstring request_json
) {
    (void)thiz;
    const char *req = request_json ? (*env)->GetStringUTFChars(env, request_json, NULL) : NULL;
    char *res = rap2p_android_invoke_json(req);
    if (request_json && req) {
        (*env)->ReleaseStringUTFChars(env, request_json, req);
    }
    jstring out = (*env)->NewStringUTF(
        env,
        res ? res : "{\"ok\":false,\"error\":\"null native response\"}"
    );
    rap2p_android_free_string(res);
    return out;
}
