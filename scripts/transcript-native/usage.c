// Called only with this isolated host's PID or its WKWebView's own PID.
#include <libproc.h>
#include <sys/resource.h>
#include <stdint.h>
int transcript_process_usage(int pid, uint64_t *resident, uint64_t *footprint) {
    struct rusage_info_v4 info = {0};
    if (proc_pid_rusage(pid, RUSAGE_INFO_V4, (rusage_info_t *)&info) != 0) return -1;
    *resident = info.ri_resident_size;
    *footprint = info.ri_phys_footprint;
    return 0;
}
