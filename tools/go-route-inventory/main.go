// Command go-route-inventory captures Gin routes without opening a listener or a database.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"sort"

	"github.com/flipped-aurora/gin-vue-admin/server/global"
	"github.com/flipped-aurora/gin-vue-admin/server/initialize"
	"github.com/gin-gonic/gin"
	"go.uber.org/zap"
)

type route struct {
	Method  string `json:"method"`
	Path    string `json:"path"`
	Handler string `json:"handler"`
}

func main() {
	output := flag.String("output", "", "route JSON output path (stdout when empty)")
	flag.Parse()
	gin.SetMode(gin.DebugMode)
	gin.DefaultWriter = io.Discard
	gin.DefaultErrorWriter = io.Discard
	global.GVA_LOG = zap.NewNop()
	global.GVA_CONFIG.Local.StorePath = "/uploads/file"
	router := initialize.Routers()
	routes := make([]route, 0, len(router.Routes()))
	for _, entry := range router.Routes() {
		routes = append(routes, route{entry.Method, entry.Path, entry.Handler})
	}
	sort.Slice(routes, func(i, j int) bool {
		if routes[i].Path == routes[j].Path {
			return routes[i].Method < routes[j].Method
		}
		return routes[i].Path < routes[j].Path
	})
	var destination io.Writer = os.Stdout
	if *output != "" {
		file, err := os.Create(*output)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		defer file.Close()
		destination = file
	}
	encoder := json.NewEncoder(destination)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(routes); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
